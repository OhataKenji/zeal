//! Runs build/test/run under a PTY; ad-hoc queries use stdout/stderr pipes.
//! Terminal output is saved independently of the Console view.
//! Build history is received by the app-owned BES, independently of process exit.
//!
//! Stop for an owned invocation uses its process group: SIGINT, then SIGKILL
//! after the shared grace period. An unknown exit releases the process slot.
//! Only the invocation's own entry may be signalled; descendants are not
//! supervised.

use crate::runargs::{argv, parse_arguments, validate_cmd, validate_pattern};
use crate::state::{
    ActiveInvocation, Core, CoreState, OwnedInvocation, ProcessDiagnostic, now_millis,
};
use rustix::process::{self, Pid, Signal};
#[cfg(test)]
use std::fs;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Time SIGINT gets to end the invocation before its group is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// Time SIGKILL gets to end the invocation before its exit is called unknown
/// and its process slot is released.
const STOP_CONFIRM: Duration = Duration::from_secs(2);
/// Only the tail of a released invocation's output is retained for its
/// diagnostic.
const DIAGNOSTIC_TAIL: u64 = 16 * 1024;
/// The diagnostic cache bound: the newest completions win.
const DIAGNOSTICS_MAX: usize = 32;
/// App exit gives live children at most `EXIT_GRACE` of polling before
/// SIGKILL, shared across every owned invocation. A reaped child's in-progress
/// save is joined separately.
const EXIT_GRACE: Duration = Duration::from_secs(1);
const EXIT_POLL: Duration = Duration::from_millis(25);

/// Shown when the group was killed but no exit was confirmed.
const STOP_UNCONFIRMED: &str = "Could not confirm the stop";

pub(crate) enum Output {
    Terminal(File),
    Pipes,
}

/// What one spawned operation is: where it runs and what it runs. The id is
/// the invocation map key, so it must be fresh for every launch.
pub(crate) struct InvocationMeta<'a> {
    pub workspace: &'a Path,
    pub cmd: &'a str,
    pub pattern: &'a str,
    pub id: &'a str,
}

/// Spawn `command` as the owned invocation `meta` describes and register it
/// under the same lock, so shutdown cannot miss an owned child. Only shutdown
/// and a duplicate id are rejected; other active invocations never block.
pub(crate) fn spawn(
    state: &mut CoreState,
    command: &mut Command,
    meta: InvocationMeta<'_>,
    output: Output,
) -> io::Result<(Child, Option<crate::terminal::Capture>)> {
    if state.shutting_down.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "application is shutting down",
        ));
    }
    if state.invocations.contains_key(meta.id) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "invocation id already in use",
        ));
    }
    let (child, terminal, capture, console) = match output {
        Output::Terminal(log) => {
            let (child, terminal, capture) = crate::terminal::spawn(command, &log)?;
            (child, Some(terminal), Some(capture), Some(log))
        }
        Output::Pipes => (
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .spawn()?,
            None,
            None,
            None,
        ),
    };
    state.invocations.insert(
        meta.id.to_string(),
        OwnedInvocation {
            workspace: meta.workspace.to_path_buf(),
            cmd: meta.cmd.to_string(),
            pattern: meta.pattern.to_string(),
            started_at: now_millis(),
            process: ActiveInvocation {
                terminal,
                pgid: Some(child.id() as i32),
                stop_started: None,
            },
            console,
        },
    );
    Ok((child, capture))
}

/// Start an invocation in `ws` and return its id. The invocation finishes in
/// the background; target data is refreshed only on request.
pub fn start(
    app: AppHandle,
    core: Arc<Core>,
    ws: &Path,
    cmd: &str,
    pattern: &str,
    arguments: &str,
) -> Result<String, String> {
    validate_cmd(cmd)?;
    validate_pattern(pattern)?;
    let arguments = parse_arguments(arguments)?;
    let mut st = core.state.lock().expect("application state lock poisoned");
    if !st.workspaces.contains_key(ws) {
        return Err("workspace is not open".to_string());
    }
    let bazel = st.bazel.clone().ok_or_else(|| "no bazel".to_string())?;
    let endpoint = receiver_endpoint(&st)?.to_string();
    // Held across the spawn so registration and shutdown stay one atomic step.
    let id = uuid::Uuid::new_v4().to_string();
    let log = crate::terminal::session_log().map_err(|e| e.to_string())?;
    // Its own process group: a stop reaches bazel and whatever it started.
    let (mut child, capture) = spawn(
        &mut st,
        Command::new(&bazel)
            .args(argv(cmd, pattern, &arguments, &endpoint, &id))
            .current_dir(ws),
        InvocationMeta {
            workspace: ws,
            cmd,
            pattern,
            id: &id,
        },
        Output::Terminal(log),
    )
    .map_err(|e| format!("failed to spawn {}: {e}", bazel.display()))?;
    let capture = capture.expect("terminal output capture");
    let result = id.clone();
    drop(st);
    core.emit(&app);

    thread::spawn(move || {
        let _guard = InvocationGuard {
            core: &core,
            id: &id,
            emit: || core.emit(&app),
        };
        // Drop capture before the guard on unwind, so Console completion can
        // never precede the capture worker's final drain.
        let mut capture = capture;
        let waited = wait(&core, &id, || poll_capture(&mut child, &capture));
        if let Err(error) = &waited {
            log::warn!("Could not confirm invocation exit: {error}");
        }
        if let Err(error) = capture.finish() {
            log::warn!("Failed to capture terminal output: {error}");
        }
        let data = app.path().app_data_dir().ok();
        match remember_diagnostic(&core, data.as_deref(), &id, retain_wait_result(&waited)) {
            Ok(true) => crate::emit_invocations_changed(&app),
            Ok(false) => {}
            Err(error) => log::warn!("Failed to retain invocation diagnostics: {error}"),
        }
    });
    Ok(result)
}

/// The process exit recorded for a diagnostic: `None` when the wait failed
/// or returned the unknown sentinel rather than a real exit code.
fn retain_wait_result(result: &io::Result<i32>) -> Option<i32> {
    result.as_ref().ok().copied().filter(|code| *code >= 0)
}

/// Retain a finished terminal invocation's metadata and bounded output tail
/// when no readable build-event record exists for it. Saved history
/// supersedes the diagnostic, so a matching record owned by the same
/// workspace skips retention; a store failure never loses the terminal
/// facts. The entry's metadata and output handle are cloned under one lock
/// hold; all I/O runs after it is dropped. Returns true only when a
/// diagnostic was inserted.
fn remember_diagnostic(
    core: &Core,
    data: Option<&Path>,
    id: &str,
    process_exit_code: Option<i32>,
) -> io::Result<bool> {
    let (mut diagnostic, file) = {
        let st = core.state.lock().expect("application state lock poisoned");
        let Some(invocation) = st.invocations.get(id) else {
            return Ok(false);
        };
        let Some(console) = invocation.console.as_ref() else {
            return Ok(false);
        };
        (
            ProcessDiagnostic {
                id: id.to_string(),
                workspace: invocation.workspace.to_string_lossy().into_owned(),
                cmd: invocation.cmd.clone(),
                pattern: invocation.pattern.clone(),
                started_at: invocation.started_at,
                process_exit_code,
                output: String::new(),
            },
            console.try_clone()?,
        )
    };
    if let Some(data) = data {
        match crate::store::read_record(data, Path::new(&diagnostic.workspace), id) {
            // Only this workspace's own record supersedes the diagnostic; the
            // repo-scoped read can surface a sibling workspace's record.
            Ok(record) if record.workspace.as_deref() == Some(diagnostic.workspace.as_str()) => {
                return Ok(false);
            }
            Ok(_) => {}
            Err(error) if error.kind == crate::store::ErrorKind::Missing => {}
            Err(error) => log::warn!("Could not check invocation history: {error}"),
        }
    }
    let length = file.metadata()?.len();
    let start = length.saturating_sub(DIAGNOSTIC_TAIL);
    let mut tail = vec![0; (length - start) as usize];
    file.read_exact_at(&mut tail, start)?;
    diagnostic.output = String::from_utf8_lossy(&tail).into_owned();
    let mut st = core.state.lock().expect("application state lock poisoned");
    st.diagnostics.push_front(diagnostic);
    st.diagnostics.truncate(DIAGNOSTICS_MAX);
    Ok(true)
}

/// A capture failure must stop the still-owned child before its output blocks.
fn poll_capture(
    child: &mut std::process::Child,
    capture: &crate::terminal::Capture,
) -> io::Result<Option<i32>> {
    let status = child.try_wait()?;
    if status.is_none() && capture.failed() {
        let _ = signal_group(child.id() as i32, Signal::KILL);
    }
    Ok(status.map(|status| status.code().unwrap_or(-1)))
}

fn receiver_endpoint(st: &CoreState) -> Result<&str, String> {
    st.bes_endpoint.as_deref().ok_or_else(|| {
        st.bes_error
            .clone()
            .unwrap_or_else(|| "BES receiver is not ready".to_string())
    })
}

/// Stop the owned invocation `id` in `ws` through the process-group lifecycle.
pub fn stop(app: AppHandle, core: Arc<Core>, ws: &Path, id: &str) -> Result<(), String> {
    begin_stop(
        &mut core.state.lock().expect("application state lock poisoned"),
        ws,
        id,
    )?;
    core.emit(&app);
    Ok(())
}

/// Signal the invocation's group once. A foreign entry is rejected; a
/// missing, repeated, or already-reaped entry is an idempotent no-op so a
/// stale Stop can never fail or signal a recycled group.
fn begin_stop(st: &mut CoreState, ws: &Path, id: &str) -> Result<(), String> {
    let Some(invocation) = st.invocations.get_mut(id) else {
        return Ok(());
    };
    if invocation.workspace != ws {
        return Err("invocation belongs to another workspace".to_string());
    }
    interrupt(&mut invocation.process);
    Ok(())
}

fn interrupt(process: &mut ActiveInvocation) {
    let Some(pgid) = process.pgid.filter(|_| process.stop_started.is_none()) else {
        return;
    };
    process.stop_started = Some(Instant::now());
    if let Err(e) = signal_process(process, pgid, Signal::INT) {
        log::warn!("SIGINT to invocation group failed: {e}");
    }
}

fn wait(core: &Core, id: &str, poll: impl FnMut() -> io::Result<Option<i32>>) -> io::Result<i32> {
    observe_process(core, id, poll, STOP_GRACE, STOP_CONFIRM)
}

pub(crate) fn wait_query(core: &Core, id: &str, child: &mut Child) -> Result<i32, String> {
    let waited = wait(core, id, || {
        child
            .try_wait()
            .map(|status| status.map(|status| status.code().unwrap_or(-1)))
    });
    let st = core.state.lock().expect("application state lock poisoned");
    let stopping = st
        .invocations
        .get(id)
        .is_some_and(|invocation| invocation.process.stop_started.is_some());
    drop(st);
    let exit = waited.map_err(|error| format!("failed to wait on query: {error}"))?;
    if stopping {
        Err("Query stopped".to_string())
    } else {
        Ok(exit)
    }
}

/// Poll and signal under the same lock. Once try_wait reaps the child, its PID
/// is revoked immediately, before saving or waking an app-exit waiter.
#[tracing::instrument(target = "zeal::operations", name = "bazel.wait", skip_all)]
fn observe_process(
    core: &Core,
    id: &str,
    mut poll: impl FnMut() -> io::Result<Option<i32>>,
    grace: Duration,
    confirm: Duration,
) -> io::Result<i32> {
    let mut killed = false;
    loop {
        let mut st = core.state.lock().expect("application state lock poisoned");
        let Some(process) = st
            .invocations
            .get_mut(id)
            .map(|invocation| &mut invocation.process)
        else {
            return Err(io::Error::other("invocation no longer owned"));
        };
        match poll() {
            Ok(Some(code)) => {
                process.pgid = None;
                return Ok(code);
            }
            Ok(None) => (),
            Err(error) => {
                // A failed wait can mean another reaper already consumed the
                // child. Its former PID is no longer safe to signal.
                process.pgid = None;
                return Err(error);
            }
        }
        if let Some(started) = process.stop_started {
            if started.elapsed() >= grace && !killed {
                if let Some(pgid) = process.pgid
                    && let Err(error) = signal_process(process, pgid, Signal::KILL)
                {
                    log::warn!("SIGKILL to invocation group failed: {error}");
                }
                killed = true;
            }
            if started.elapsed() >= grace + confirm {
                process.pgid = None;
                return Err(io::Error::new(io::ErrorKind::TimedOut, STOP_UNCONFIRMED));
            }
        }
        drop(st);
        thread::sleep(EXIT_POLL);
    }
}

/// A panic in observation or saving must release the process slot and wake app
/// exit. Normal completion publishes after the slot is released.
pub(crate) struct InvocationGuard<'a, F: Fn()> {
    pub(crate) core: &'a Core,
    pub(crate) id: &'a str,
    pub(crate) emit: F,
}
impl<F: Fn()> Drop for InvocationGuard<'_, F> {
    fn drop(&mut self) {
        let mut st = self
            .core
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // The owning worker finished: release the process slot. The entry
        // leaves the map entirely, including after a panic, so a finished
        // invocation's console and process are gone together.
        st.invocations.remove(self.id);
        self.core.invocation_finished.notify_all();
        drop(st);
        // Avoid a second panic from an event emission while unwinding.
        if !thread::panicking() {
            (self.emit)();
        }
    }
}

/// Signal every still-owned process group during app exit. `pgid` was
/// revoked at reap, so this never addresses a recycled group.
fn kill_invocation_groups(st: &CoreState) {
    for invocation in st.invocations.values() {
        let Some(pgid) = invocation.process.pgid else {
            continue;
        };
        if let Err(error) = signal_process(&invocation.process, pgid, Signal::KILL) {
            log::warn!("SIGKILL to invocation group failed: {error}");
        }
    }
}

/// The exit deadline passed without a confirmed stop: revoke every group's
/// signal ownership so no later stop can address a possibly recycled pid.
fn revoke_unconfirmed_groups(st: &mut CoreState) {
    for invocation in st.invocations.values_mut() {
        invocation.process.pgid = None;
    }
}

/// Bound process stopping to the short exit grace, shared across every owned
/// invocation and in-flight modules operation: one common deadline, not one
/// per child. Children already reaped have nothing left to signal; their
/// in-progress saves and modules guards are joined through the condvar. New
/// starts are rejected by `shutting_down`.
pub fn exit_stop(core: &Core) {
    {
        let mut st = core.state.lock().expect("application state lock poisoned");
        st.shutting_down.store(true, Ordering::Relaxed);
        for invocation in st.invocations.values_mut() {
            interrupt(&mut invocation.process);
        }
        if st.invocations.is_empty() && st.modules_running.is_empty() {
            return;
        }
    }
    let kill_at = Instant::now() + EXIT_GRACE;
    let finish_at = kill_at + STOP_CONFIRM;
    let mut killed = false;
    loop {
        let mut st = core.state.lock().expect("application state lock poisoned");
        if st.invocations.is_empty() && st.modules_running.is_empty() {
            return;
        }
        let now = Instant::now();
        if now >= kill_at && !killed {
            kill_invocation_groups(&st);
            killed = true;
        }
        if now >= finish_at {
            revoke_unconfirmed_groups(&mut st);
            return;
        }
        let _ = core
            .invocation_finished
            .wait_timeout(st, EXIT_POLL)
            .unwrap_or_else(|error| error.into_inner());
    }
}

/// An invocation's process group must not address the caller's group (0) or
/// all signalable processes (1 when negated by kill_process_group).
pub(crate) fn process_group(pgid: i32) -> io::Result<Pid> {
    if pgid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid invocation process group",
        ));
    }
    Ok(Pid::from_raw(pgid).expect("invocation process group is positive"))
}

/// Signal the invocation's session group, or its terminal's foreground job
/// group.
pub(crate) fn signal_group(pgid: i32, signal: Signal) -> io::Result<()> {
    Ok(process::kill_process_group(process_group(pgid)?, signal)?)
}

/// Signal `process`'s session group, plus its terminal's foreground job
/// group when that differs — the same reach the single slot had.
fn signal_process(process: &ActiveInvocation, pgid: i32, signal: Signal) -> io::Result<()> {
    if let Some(foreground) = process
        .terminal
        .as_ref()
        .and_then(crate::terminal::Control::foreground_group)
        && foreground != pgid
    {
        let _ = signal_group(foreground, signal);
    }
    signal_group(pgid, signal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::OwnedInvocation;
    use crate::store;
    use crate::store::events::test_support::Fixture;
    use std::path::{Path, PathBuf};

    const WS: &str = "/ws";

    fn register_ws(st: &mut CoreState, ws: &Path) {
        st.workspaces.entry(ws.to_path_buf()).or_default();
    }

    /// The state `start` leaves around a spawned child: its invocation entry
    /// owns the process and a console session under the same id.
    fn hold_invocation(
        core: &Core,
        ws: &Path,
        id: &str,
        terminal: crate::terminal::Control,
        pgid: i32,
        log: std::fs::File,
    ) {
        let ws = &ws.canonicalize().unwrap_or_else(|_| ws.to_path_buf());
        let mut state = core.state.lock().unwrap();
        register_ws(&mut state, ws);
        state.invocations.insert(
            id.to_string(),
            OwnedInvocation {
                workspace: ws.to_path_buf(),
                cmd: "build".to_string(),
                pattern: "//pkg:b".to_string(),
                started_at: 1,
                process: ActiveInvocation {
                    terminal: Some(terminal),
                    pgid: Some(pgid),
                    stop_started: None,
                },
                console: Some(log),
            },
        );
    }

    /// Finish what a worker's guard drop does once observation ended: the
    /// entry leaves the map and wakes any app-exit waiter.
    fn release_process(core: &Core, id: &str) {
        drop(InvocationGuard {
            core,
            id,
            emit: || (),
        });
    }

    /// Run the production retention path for a stub invocation whose console
    /// already holds `bytes`, then release the entry like a finished worker.
    fn remember_output(
        core: &Core,
        ws: &Path,
        id: &str,
        bytes: &[u8],
        data: Option<&Path>,
    ) -> bool {
        let mut log = crate::terminal::session_log().unwrap();
        log.write_all(bytes).unwrap();
        hold_invocation(
            core,
            ws,
            id,
            crate::terminal::Control::stub(),
            i32::MAX,
            log,
        );
        let retained = remember_diagnostic(core, data, id, Some(9)).unwrap();
        release_process(core, id);
        retained
    }

    /// Spawn through the production path so the invocation owns its process,
    /// terminal and console exactly as `start` leaves them.
    fn spawn_gui_child(
        core: &Core,
        command: &mut Command,
        meta: InvocationMeta<'_>,
    ) -> (Child, crate::terminal::Capture, std::fs::File) {
        let log = crate::terminal::session_log().unwrap();
        let read_log = log.try_clone().unwrap();
        let mut st = core.state.lock().unwrap();
        register_ws(&mut st, meta.workspace);
        let (child, capture) =
            spawn(&mut st, command, meta, Output::Terminal(log)).expect("gui child spawn");
        drop(st);
        (child, capture.expect("terminal capture"), read_log)
    }

    fn console_text(core: &Core, ws: &Path, id: &str) -> String {
        let console =
            serde_json::to_value(crate::console::read(core, ws.to_str().unwrap(), id, 0).unwrap())
                .unwrap();
        let bytes: Vec<u8> = serde_json::from_value(console["data"].clone()).unwrap();
        String::from_utf8(bytes).unwrap()
    }

    fn wait_for_console_marker(core: &Core, ws: &Path, id: &str, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !console_text(core, ws, id).contains(marker) {
            assert!(Instant::now() < deadline, "{id} never printed {marker}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Every entry in the map owns a live worker: release removes it.
    fn assert_drained(core: &Core, ids: &[&str]) {
        let st = core.state.lock().unwrap();
        for id in ids {
            assert!(!st.invocations.contains_key(*id), "{id} still owned");
        }
    }

    fn assert_no_foreign_marker(id: &str, text: &str, entries: &[(&str, &str)], own: usize) {
        for (position, (_, other_marker)) in entries.iter().enumerate() {
            assert!(
                position == own || !text.contains(other_marker),
                "{id} contains {other_marker}: {text}"
            );
        }
    }

    fn assert_group_dead(pgid: i32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while group_alive(pgid) {
            assert!(Instant::now() < deadline, "group {pgid} still has members");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// An owned process without console output, as queries register.
    fn hold_process(core: &Core, ws: &Path, id: &str, pgid: Option<i32>) {
        let ws = &ws.canonicalize().unwrap_or_else(|_| ws.to_path_buf());
        let mut state = core.state.lock().unwrap();
        register_ws(&mut state, ws);
        state.invocations.insert(
            id.to_string(),
            OwnedInvocation {
                workspace: ws.to_path_buf(),
                cmd: "query".to_string(),
                pattern: "//pkg:*".to_string(),
                started_at: 1,
                process: ActiveInvocation {
                    terminal: None,
                    pgid,
                    stop_started: None,
                },
                console: None,
            },
        );
    }

    fn held_pgid(st: &CoreState, id: &str) -> Option<i32> {
        st.invocations
            .get(id)
            .and_then(|invocation| invocation.process.pgid)
    }

    fn is_stopping(st: &CoreState, id: &str) -> bool {
        st.invocations
            .get(id)
            .is_some_and(|invocation| invocation.process.stop_started.is_some())
    }

    fn process_count(st: &CoreState) -> usize {
        st.invocations.len()
    }

    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc;

    struct LogDir(std::path::PathBuf);

    impl LogDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("zeal-invocation-log-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for LogDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn log_bytes(log: &fs::File) -> Vec<u8> {
        use std::os::unix::fs::FileExt;
        let mut bytes = vec![0; log.metadata().unwrap().len() as usize];
        log.read_exact_at(&mut bytes, 0).unwrap();
        bytes
    }

    #[test]
    fn terminal_saves_both_streams_and_non_utf8_bytes() {
        let log = crate::terminal::session_log().unwrap();
        let (mut child, _terminal, mut capture) = crate::terminal::spawn(
            Command::new("sh")
                .arg("-c")
                .arg("printf 'out\\r\\n'; printf 'err\\n' >&2; printf '\\377tail'"),
            &log,
        )
        .unwrap();
        let _guard = GroupGuard(child.id() as i32);
        assert!(child.stdout.is_none());
        assert!(child.stderr.is_none());
        assert!(child.wait().unwrap().success());
        capture.finish().unwrap();
        assert_eq!(log_bytes(&log), b"out\r\r\nerr\r\n\xfftail");
        assert_eq!(
            String::from_utf8_lossy(&log_bytes(&log)),
            "out\r\r\nerr\r\n\u{fffd}tail"
        );
    }

    #[test]
    fn parent_exit_does_not_wait_for_a_descendant_holding_the_log_open() {
        let log = crate::terminal::session_log().unwrap();
        let (mut child, _terminal, mut capture) = crate::terminal::spawn(
            Command::new("sh")
                .arg("-c")
                .arg("trap '' HUP; sleep 30 & printf 'done\\n'; exit 7"),
            &log,
        )
        .unwrap();
        let pgid = child.id() as i32;
        let _guard = GroupGuard(pgid);
        let (tx, rx) = mpsc::channel();
        let waiter = thread::spawn(move || tx.send(child.wait().unwrap()).unwrap());
        let status = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("wait must not depend on descendant output");
        waiter.join().unwrap();
        assert_eq!(status.code(), Some(7));
        assert!(
            group_alive(pgid),
            "descendant should still retain its output file"
        );
        capture.finish().unwrap();
        assert_eq!(String::from_utf8_lossy(&log_bytes(&log)), "done\r\n");
    }

    #[test]
    fn argv_has_the_command_one_pattern_and_the_observation_flags() {
        for (cmd, pattern) in [("build", "//pkg:b"), ("test", "//pkg:t")] {
            let args = argv(cmd, pattern, &[], "grpc://127.0.0.1:12345", "abc");
            assert_eq!(args[0], "--noblock_for_lock");
            assert_eq!(args[1], cmd);
            assert_eq!(args.last().unwrap(), pattern);
            assert_eq!(
                &args[2..args.len() - 1],
                [
                    "--bes_backend=grpc://127.0.0.1:12345",
                    "--bes_proxy=",
                    "--bes_upload_mode=wait_for_upload_complete",
                    "--tool_tag=zeal",
                    "--color=yes",
                    "--curses=no",
                    "--invocation_id=abc",
                ],
                "{cmd}"
            );
        }
    }

    #[test]
    fn receiver_readiness_is_required_before_and_after_server_termination() {
        let core = Core::new();
        let mut state = core.state.lock().unwrap();
        assert_eq!(
            receiver_endpoint(&state).unwrap_err(),
            "BES receiver is not ready"
        );
        state.bes_endpoint = Some("grpc://127.0.0.1:12345".into());
        assert_eq!(receiver_endpoint(&state).unwrap(), "grpc://127.0.0.1:12345");
        state.bes_endpoint = None;
        state.bes_error = Some("BES receiver stopped".into());
        assert_eq!(
            receiver_endpoint(&state).unwrap_err(),
            "BES receiver stopped"
        );
    }

    /// A terminal completion without a readable build-event record keeps a
    /// bounded diagnostic: the history accessor reports its metadata and
    /// output tail for its own workspace only, even without a database.
    #[test]
    fn failed_process_releases_entry_and_retains_a_readable_diagnostic() {
        let temp = LogDir::new();
        let ws = temp.0.join("workspace");
        fs::create_dir(&ws).unwrap();
        let ws = ws.canonicalize().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let core = Arc::new(Core::new());
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf 'Another command is running. Exiting immediately.\\n' >&2; exit 9",
        ]);
        let (child, capture, _log) = spawn_gui_child(
            &core,
            &mut command,
            InvocationMeta {
                workspace: &ws,
                cmd: "build",
                pattern: "//:b",
                id: &id,
            },
        );
        let code = complete_gui_run(child, capture, core.clone(), &ws, id.clone(), None, None);
        assert_eq!(code, 9);
        let st = core.state.lock().unwrap();
        let diagnostics = st.diagnostics_for(&ws);
        let [d] = diagnostics.as_slice() else {
            panic!("expected one diagnostic")
        };
        assert_eq!((&d.id, d.process_exit_code), (&id, Some(9)));
        assert!(
            d.output
                .contains("Another command is running. Exiting immediately.")
        );
        assert!(st.diagnostics_for(&ws.with_file_name("other")).is_empty());
    }

    /// Only a real exit status becomes the diagnostic's process exit code.
    #[test]
    fn retain_wait_result_keeps_only_real_exit_codes() {
        assert_eq!(
            retain_wait_result(&Err(io::Error::other("wait failed"))),
            None
        );
        assert_eq!(retain_wait_result(&Ok(-1)), None);
        assert_eq!(retain_wait_result(&Ok(0)), Some(0));
        assert_eq!(retain_wait_result(&Ok(9)), Some(9));
    }

    /// The retained tail is the file's last DIAGNOSTIC_TAIL bytes; a split
    /// UTF-8 sequence decodes lossily, so the string can exceed the window.
    #[test]
    fn remember_diagnostic_reads_the_bounded_tail_lossily() {
        let core = Core::new();
        let ws = Path::new(WS);
        let id = "tail-boundary";
        // 'Z' + é (two bytes) + TAIL-1 ASCII: the window opens on é's
        // second byte, which cannot start a UTF-8 sequence.
        let mut bytes = vec![b'Z', 0xC3, 0xA9];
        bytes.resize(2 + DIAGNOSTIC_TAIL as usize, b'A');
        assert!(remember_output(&core, ws, id, &bytes, None));
        let st = core.state.lock().unwrap();
        let diagnostics = st.diagnostics_for(ws);
        let [d] = diagnostics.as_slice() else {
            panic!("expected one diagnostic")
        };
        assert_eq!(
            d.output,
            format!("\u{FFFD}{}", "A".repeat(DIAGNOSTIC_TAIL as usize - 1))
        );
        assert!(d.output.len() > DIAGNOSTIC_TAIL as usize);
    }

    /// The cache keeps the newest DIAGNOSTICS_MAX diagnostics, newest first.
    #[test]
    fn diagnostics_evict_the_oldest_beyond_the_bound() {
        let core = Core::new();
        let ws = Path::new(WS);
        for i in 0..=DIAGNOSTICS_MAX {
            assert!(remember_output(&core, ws, &format!("d{i}"), b"out", None));
        }
        let st = core.state.lock().unwrap();
        let diagnostics = st.diagnostics_for(ws);
        assert_eq!(diagnostics.len(), DIAGNOSTICS_MAX);
        assert_eq!(diagnostics[0].id, format!("d{DIAGNOSTICS_MAX}"));
        assert_eq!(diagnostics[DIAGNOSTICS_MAX - 1].id, "d1");
    }

    /// Repo scope is not ownership: a record owned by this workspace
    /// suppresses retention, while a sibling's same-id record does not.
    #[test]
    fn remember_diagnostic_only_yields_to_the_same_workspaces_record() {
        let f = Fixture::new();
        let mut sibling = Fixture::new();
        sibling.data = f.data.clone();
        crate::workspace::test_support::repo(&f.ws, "https://example.com/shared.git");
        crate::workspace::test_support::repo(&sibling.ws, "https://example.com/shared.git");
        store::refresh_workspace(&f.data, &f.ws, Some("https://example.com/shared.git")).unwrap();
        store::refresh_workspace(&f.data, &sibling.ws, Some("https://example.com/shared.git"))
            .unwrap();
        f.complete();
        sibling.complete();
        let ws = f.ws.canonicalize().unwrap();
        let core = Core::new();
        let data = f.data.as_path();

        let own = f.stream.invocation_id.clone();
        assert!(!remember_output(&core, &ws, &own, b"out", Some(data)));
        assert!(core.state.lock().unwrap().diagnostics_for(&ws).is_empty());

        let sibling_id = sibling.stream.invocation_id.clone();
        assert!(remember_output(&core, &ws, &sibling_id, b"out", Some(data)));
        let st = core.state.lock().unwrap();
        let diagnostics = st.diagnostics_for(&ws);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].id, sibling_id);
    }

    /// A stale worker can never affect another invocation: release removes
    /// only the id its guard names.
    #[test]
    fn release_removes_only_the_named_invocation() {
        let core = Core::new();
        let ws = PathBuf::from(WS);
        {
            let mut state = core.state.lock().unwrap();
            register_ws(&mut state, &ws);
            state.invocations.insert(
                "new".to_string(),
                OwnedInvocation {
                    workspace: ws.clone(),
                    cmd: "build".to_string(),
                    pattern: "//:b".to_string(),
                    started_at: 1,
                    process: ActiveInvocation::stub(Some(42)),
                    console: Some(crate::terminal::session_log().unwrap()),
                },
            );
            state.invocations.insert(
                "other".to_string(),
                OwnedInvocation {
                    workspace: ws.clone(),
                    cmd: "build".to_string(),
                    pattern: "//:b".to_string(),
                    started_at: 1,
                    process: ActiveInvocation::stub(Some(43)),
                    console: None,
                },
            );
        }
        // A foreign id's guard cannot release "new".
        release_process(&core, "old");
        {
            let st = core.state.lock().unwrap();
            assert!(st.invocations.contains_key("new"));
            assert!(st.invocations.contains_key("other"));
        }
        release_process(&core, "new");
        let st = core.state.lock().unwrap();
        assert!(!st.invocations.contains_key("new"));
        assert!(st.invocations.contains_key("other"));
    }

    /// Uses only a temporary workspace, rc file, data directory and Bazel root.
    /// The rc deliberately points at another backend and an invalid Unix proxy.
    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary"]
    async fn real_bazel_gui_flags_build_test_and_run_with_input() {
        let fixture = GuiBazelFixture::new().await;
        check_gui_run(&fixture, "build", "//:double").await;
        check_gui_run(&fixture, "test", "//:check").await;
        check_gui_run(&fixture, "run", "//:double").await;
        assert_eq!(
            store::list(&fixture.data, &fixture.ws, &[]).unwrap().len(),
            3
        );
    }

    struct GuiBazelFixture {
        temp: LogDir,
        ws: std::path::PathBuf,
        data: std::path::PathBuf,
        bazel: String,
        endpoint: String,
        server: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    }
    impl Drop for GuiBazelFixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    impl GuiBazelFixture {
        async fn new() -> Self {
            let bazel = std::env::var("ZEAL_TEST_BAZEL").expect("ZEAL_TEST_BAZEL");
            let temp = LogDir::new();
            let ws = temp.0.join("workspace");
            let data = temp.0.join("data");
            fs::create_dir(&ws).unwrap();
            let ws = ws.canonicalize().unwrap();
            fs::write(ws.join("WORKSPACE"), "workspace(name='zeal_gui_test')\n").unwrap();
            fs::write(ws.join(".bazelrc"), "build --bes_proxy=unix:/zeal-test-no-proxy.sock\nbuild --bes_backend=grpc://127.0.0.1:1\n").unwrap();
            fs::write(ws.join("BUILD"), "load(':binary.bzl','binary','check_test')\nplatform(name='host')\nbinary(name='double')\ncheck_test(name='check')\n").unwrap();
            fs::write(ws.join("binary.bzl"), "def _binary(ctx):\n    out=ctx.actions.declare_file(ctx.label.name+'.py')\n    ctx.actions.write(out,'#!/usr/bin/env python3\\nprint(int(input())*2)\\nraise SystemExit(23)\\n',is_executable=True)\n    return [DefaultInfo(executable=out)]\ndef _check(ctx):\n    out=ctx.actions.declare_file(ctx.label.name+'.sh')\n    ctx.actions.write(out,'#!/bin/sh\\nexit 0\\n',is_executable=True)\n    return [DefaultInfo(executable=out)]\nbinary=rule(implementation=_binary,executable=True)\ncheck_test=rule(implementation=_check,test=True)\n").unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let endpoint = format!("grpc://{address}");
            let server = tokio::spawn(crate::bes::serve(listener, data.clone()));
            Self {
                temp,
                ws,
                data,
                bazel,
                endpoint,
                server,
            }
        }
        fn command(&self, cmd: &str, pattern: &str, id: &str) -> Command {
            self.command_at(&self.ws, &self.temp.0.join("bazel"), cmd, pattern, id)
        }
        fn command_at(
            &self,
            ws: &Path,
            root: &Path,
            cmd: &str,
            pattern: &str,
            id: &str,
        ) -> Command {
            let mut command = Command::new(&self.bazel);
            command
                .current_dir(ws)
                .args(["--batch", "--nosystem_rc", "--nohome_rc"])
                .arg(format!("--output_user_root={}", root.display()))
                .args(argv(cmd, pattern, &[], &self.endpoint, id))
                .args([
                    "--enable_bzlmod=false",
                    "--enable_workspace=true",
                    "--incompatible_autoload_externally=",
                    "--host_platform=//:host",
                    "--platforms=//:host",
                    "--spawn_strategy=local",
                    "--bes_timeout=10s",
                ]);
            command
        }
    }

    async fn check_gui_run(fixture: &GuiBazelFixture, cmd: &'static str, pattern: &str) {
        let id = uuid::Uuid::new_v4().to_string();
        let log = crate::terminal::session_log().unwrap();
        let read_log = log.try_clone().unwrap();
        let mut command = fixture.command(cmd, pattern, &id);
        let (child, terminal, capture) = crate::terminal::spawn(&mut command, &log).unwrap();
        let core = Arc::new(Core::new());
        hold_invocation(&core, &fixture.ws, &id, terminal, child.id() as i32, log);
        let (release, input) = mpsc::sync_channel(1);
        let owner_core = core.clone();
        let owner_id = id.clone();
        let owner_ws = fixture.ws.clone();
        let owner_data = fixture.data.clone();
        let process = tokio::task::spawn_blocking(move || {
            complete_gui_run(
                child,
                capture,
                owner_core,
                &owner_ws,
                owner_id,
                (cmd == "run").then_some(input),
                Some(owner_data),
            )
        });
        // A `run` child blocks on stdin: feed it only once the build's BEP
        // record exists, the order the GUI observes.
        if cmd == "run" {
            wait_for_gui_build(fixture, &fixture.ws, &id, &read_log, &process).await;
            crate::terminal::resize(&core, fixture.ws.to_str().unwrap(), &id, 100, 30).unwrap();
            release.send(()).unwrap();
        }
        let code = tokio::time::timeout(Duration::from_secs(60), process)
            .await
            .unwrap()
            .unwrap();
        let output = String::from_utf8(log_bytes(&read_log)).unwrap();
        assert_eq!(code, if cmd == "run" { 23 } else { 0 }, "{cmd}: {output}");
        assert_gui_record(fixture, cmd, pattern, &id);
        assert!(
            !core.state.lock().unwrap().invocations.contains_key(&id),
            "released entry must leave the map"
        );
        assert_console_unavailable(&core, &fixture.ws, &id);
        assert!(!output.is_empty(), "{cmd}: no terminal output captured");
    }

    /// The BES-recorded invocation: the build's own command, pattern and
    /// exit, plus the artifacts the GUI needs to reopen the invocation.
    fn assert_gui_record(fixture: &GuiBazelFixture, cmd: &str, pattern: &str, id: &str) {
        let record = store::read_record(&fixture.data, &fixture.ws, id).unwrap();
        assert_eq!(record.cmd.as_deref(), Some(cmd));
        assert_eq!(record.pattern.as_deref(), Some(pattern));
        assert_eq!(
            record.exit_code,
            Some(0),
            "program exit must not replace build exit"
        );
        assert!(record.finished_at.is_some());
        assert!(store::artifact(&fixture.data, &fixture.ws, id, "bep.binpb.zst").is_ok());
        assert!(store::artifact(&fixture.data, &fixture.ws, id, "profile.json.gz").is_ok());
    }

    struct GuiRun {
        id: String,
        ws: PathBuf,
        read_log: fs::File,
        release: mpsc::SyncSender<()>,
        process: tokio::task::JoinHandle<i32>,
    }

    fn second_workspace(fixture: &GuiBazelFixture) -> PathBuf {
        let ws_b = fixture.temp.0.join("workspace-b");
        fs::create_dir(&ws_b).unwrap();
        let ws_b = ws_b.canonicalize().unwrap();
        for name in ["WORKSPACE", ".bazelrc", "BUILD", "binary.bzl"] {
            fs::copy(fixture.ws.join(name), ws_b.join(name)).unwrap();
        }
        ws_b
    }

    fn start_gui_run(
        fixture: &GuiBazelFixture,
        core: &Arc<Core>,
        ws: &Path,
        root: PathBuf,
        gated: bool,
    ) -> GuiRun {
        let id = uuid::Uuid::new_v4().to_string();
        let mut command = fixture.command_at(ws, &root, "run", "//:double", &id);
        let (child, capture, read_log) = spawn_gui_child(
            core,
            &mut command,
            InvocationMeta {
                workspace: ws,
                cmd: "run",
                pattern: "//:double",
                id: &id,
            },
        );
        let (release, released) = mpsc::sync_channel::<()>(1);
        let owner_core = core.clone();
        let owner_id = id.clone();
        let owner_ws = ws.to_path_buf();
        let owner_data = fixture.data.clone();
        let process = tokio::task::spawn_blocking(move || {
            complete_gui_run(
                child,
                capture,
                owner_core,
                &owner_ws,
                owner_id,
                gated.then_some(released),
                Some(owner_data),
            )
        });
        GuiRun {
            id,
            ws: ws.to_path_buf(),
            read_log,
            release,
            process,
        }
    }

    async fn wait_for_bep_build(fixture: &GuiBazelFixture, run: &GuiRun) {
        let receipt =
            wait_for_gui_build_receipt(fixture, &run.ws, &run.id, &run.read_log, &run.process);
        if let Err(error) = tokio::time::timeout(Duration::from_secs(90), receipt).await {
            panic!(
                "BES did not finalize before stdin: {error}; {}",
                String::from_utf8_lossy(&log_bytes(&run.read_log))
            );
        }
    }

    async fn wait_for_bep_builds(fixture: &GuiBazelFixture, runs: &[GuiRun]) {
        for run in runs {
            wait_for_bep_build(fixture, run).await;
        }
    }

    fn assert_owned_groups(core: &Core, runs: &[GuiRun]) {
        let st = core.state.lock().unwrap();
        for run in runs {
            let invocation = &st.invocations[&run.id];
            assert!(
                invocation.process.pgid.is_some(),
                "{} lost signal ownership while its program waited",
                run.id
            );
            assert!(!is_stopping(&st, &run.id));
        }
    }

    fn assert_run_still_owned(core: &Core, run: &GuiRun) {
        let st = core.state.lock().unwrap();
        let invocation = &st.invocations[&run.id];
        assert!(
            invocation.process.pgid.is_some(),
            "{} lost signal ownership",
            run.id
        );
    }

    fn assert_run_records(fixture: &GuiBazelFixture, runs: &[GuiRun]) {
        for run in runs {
            let record = store::read_record(&fixture.data, &run.ws, &run.id).unwrap();
            assert_eq!(record.cmd.as_deref(), Some("run"));
            assert_eq!(record.pattern.as_deref(), Some("//:double"));
            assert_eq!(
                record.exit_code,
                Some(0),
                "program exit must not replace build exit"
            );
            assert!(record.finished_at.is_some());
        }
    }

    /// Two workspaces run interactive programs at once on one Core and one
    /// BES: stopping A leaves B's process, console and record untouched.
    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary"]
    async fn real_bazel_two_workspaces_run_and_stop_independently() {
        let fixture = GuiBazelFixture::new().await;
        let ws_b = second_workspace(&fixture);
        let core = Arc::new(Core::new());
        let mut runs = vec![
            start_gui_run(
                &fixture,
                &core,
                &fixture.ws,
                fixture.temp.0.join("bazel"),
                false,
            ),
            start_gui_run(&fixture, &core, &ws_b, fixture.temp.0.join("bazel-b"), true),
        ];
        wait_for_bep_builds(&fixture, &runs).await;
        assert_owned_groups(&core, &runs);

        let id_a = runs[0].id.clone();
        let ws_a = runs[0].ws.clone();
        begin_stop(&mut core.state.lock().unwrap(), &ws_a, &id_a).unwrap();
        let code_a = tokio::time::timeout(Duration::from_secs(30), &mut runs[0].process)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(code_a, 23, "stopped program must not exit cleanly");
        assert_run_still_owned(&core, &runs[1]);
        runs[1].release.send(()).unwrap();
        let code_b = tokio::time::timeout(Duration::from_secs(30), &mut runs[1].process)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(code_b, 23);
        assert_run_records(&fixture, &runs);
        // The core entries are gone after release; the fixture-owned log
        // handles still hold each run's captured bytes for isolation checks.
        let text_a = String::from_utf8(log_bytes(&runs[0].read_log)).unwrap();
        let text_b = String::from_utf8(log_bytes(&runs[1].read_log)).unwrap();
        assert!(text_b.contains("42\r\n"), "B console: {text_b}");
        assert!(!text_a.contains("42\r\n"), "A console: {text_a}");
        let ids: Vec<&str> = runs.iter().map(|run| run.id.as_str()).collect();
        assert_drained(&core, &ids);
    }

    /// Spawn one `build` through the production path and run its completion
    /// in the background exactly as the GUI worker does.
    fn start_gui_build(
        fixture: &GuiBazelFixture,
        core: &Arc<Core>,
        pattern: &str,
    ) -> (String, tokio::task::JoinHandle<i32>) {
        let id = uuid::Uuid::new_v4().to_string();
        let mut command = fixture.command("build", pattern, &id);
        let (child, capture, _log) = spawn_gui_child(
            core,
            &mut command,
            InvocationMeta {
                workspace: &fixture.ws,
                cmd: "build",
                pattern,
                id: &id,
            },
        );
        let finish_core = core.clone();
        let finish_ws = fixture.ws.clone();
        let finish_data = fixture.data.clone();
        let finish_id = id.clone();
        let process = tokio::task::spawn_blocking(move || {
            complete_gui_run(
                child,
                capture,
                finish_core,
                &finish_ws,
                finish_id,
                None,
                Some(finish_data),
            )
        });
        (id, process)
    }

    /// Wait until the blocking action reports its start, failing fast when
    /// the owner build already ended or the deadline passes.
    async fn wait_for_action_marker(
        core: &Core,
        ws: &Path,
        owner: &tokio::task::JoinHandle<i32>,
        owner_id: &str,
        marker: &Path,
    ) {
        let deadline = Instant::now() + Duration::from_secs(180);
        while !marker.exists() {
            assert_owner_progress(core, ws, owner, owner_id, deadline);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn assert_owner_progress(
        core: &Core,
        ws: &Path,
        owner: &tokio::task::JoinHandle<i32>,
        owner_id: &str,
        deadline: Instant,
    ) {
        assert!(
            !owner.is_finished(),
            "owner build finished before its action started"
        );
        assert!(
            Instant::now() < deadline,
            "blocking action never started: {}",
            console_text(core, ws, owner_id)
        );
    }

    /// The lock contender left the invocation map while the owner kept its
    /// entry, and only the contender gained a diagnostic.
    fn assert_contender_diagnostic(core: &Core, ws: &Path, owner_id: &str, contender_id: &str) {
        let st = core.state.lock().unwrap();
        assert!(
            st.invocations.contains_key(owner_id),
            "owner lost its entry"
        );
        assert!(!st.invocations.contains_key(contender_id));
        let diagnostics = st.diagnostics_for(ws);
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == contender_id)
            .expect("rejected contender must retain a diagnostic");
        assert_eq!(diagnostic.process_exit_code, Some(9));
        assert!(
            diagnostic.output.contains("Another command"),
            "diagnostic must keep the lock rejection: {}",
            diagnostic.output
        );
    }

    /// A contender rejected by the output-base lock exits without a build
    /// event record: its terminal facts stay readable as a diagnostic while
    /// the lock owner keeps running and finishes with its own record.
    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary"]
    async fn real_bazel_same_output_base_lock_contention_keeps_a_diagnostic() {
        let fixture = GuiBazelFixture::new().await;
        // A genrule that reports its start, then blocks on a gate file with a
        // bounded wait so a stuck test cannot hold the output base forever.
        let marker = fixture.temp.0.join("action-started");
        let gate = fixture.temp.0.join("release-gate");
        let mut build = fs::read_to_string(fixture.ws.join("BUILD")).unwrap();
        build.push_str(&format!(
            "genrule(\n    name='blocker',\n    outs=['blocker.txt'],\n    cmd=\"touch '{}' ; i=0; while [ ! -f '{}' ] && [ $$i -lt 900 ]; do sleep 0.2; i=$$((i+1)); done; [ -f '{}' ] && echo done > $@ || exit 1\",\n)\n",
            marker.display(),
            gate.display(),
            gate.display(),
        ));
        fs::write(fixture.ws.join("BUILD"), build).unwrap();
        let core = Arc::new(Core::new());
        let (owner_id, owner) = start_gui_build(&fixture, &core, "//:blocker");
        // The contender may start only once the owner holds the output base.
        wait_for_action_marker(&core, &fixture.ws, &owner, &owner_id, &marker).await;
        let (contender_id, contender) = start_gui_build(&fixture, &core, "//:double");
        let contender_code = tokio::time::timeout(Duration::from_secs(90), contender)
            .await
            .expect("contender must be rejected promptly")
            .unwrap();
        assert_eq!(
            contender_code, 9,
            "contender did not exit with Bazel's lock refusal"
        );
        // The rejection left no record; its bounded output rides on the
        // diagnostic instead of a console.
        assert_eq!(
            store::read_record(&fixture.data, &fixture.ws, &contender_id)
                .unwrap_err()
                .kind,
            store::ErrorKind::Missing
        );
        assert_contender_diagnostic(&core, &fixture.ws, &owner_id, &contender_id);
        // Releasing the gate lets the owner finish with its own record; the
        // owner's matching record means it never gains a diagnostic.
        fs::write(&gate, "").unwrap();
        let owner_code = tokio::time::timeout(Duration::from_secs(120), owner)
            .await
            .expect("owner must finish after the gate opens")
            .unwrap();
        assert_eq!(owner_code, 0);
        assert_gui_record(&fixture, "build", "//:blocker", &owner_id);
        let st = core.state.lock().unwrap();
        assert!(
            !st.diagnostics_for(&fixture.ws)
                .iter()
                .any(|diagnostic| diagnostic.id == owner_id),
            "a recorded invocation must not keep a diagnostic"
        );
        drop(st);
        assert_drained(&core, &[owner_id.as_str(), contender_id.as_str()]);
    }

    /// A released invocation's console is gone with its entry: reads and
    /// writes fail, while the fixture-owned log handle keeps the bytes.
    fn assert_console_unavailable(core: &Core, ws: &Path, id: &str) {
        assert!(
            crate::console::read(core, ws.to_str().unwrap(), id, 0).is_err(),
            "released console must not be readable"
        );
        assert!(crate::terminal::write(core, ws.to_str().unwrap(), id, b"late input").is_err());
    }

    fn complete_gui_run(
        mut child: std::process::Child,
        mut capture: crate::terminal::Capture,
        core: Arc<Core>,
        ws: &Path,
        id: String,
        input: Option<mpsc::Receiver<()>>,
        data: Option<PathBuf>,
    ) -> i32 {
        let _group = GroupGuard(child.id() as i32);
        if let Some(input) = input {
            input
                .recv_timeout(Duration::from_secs(120))
                .expect("BEP completes before stdin");
            assert!(
                child.try_wait().unwrap().is_none(),
                "program still waits for input"
            );
            crate::terminal::write(&core, ws.to_str().unwrap(), &id, b"21\n").unwrap();
        }
        let waited = observe_process(
            &core,
            &id,
            || Ok(child.try_wait()?.map(|status| status.code().unwrap_or(-1))),
            STOP_GRACE,
            STOP_CONFIRM,
        );
        capture.finish().unwrap();
        remember_diagnostic(&core, data.as_deref(), &id, retain_wait_result(&waited))
            .expect("diagnostic retention");
        release_process(&core, &id);
        waited.unwrap()
    }

    async fn wait_for_gui_build(
        fixture: &GuiBazelFixture,
        ws: &Path,
        id: &str,
        log: &fs::File,
        process: &tokio::task::JoinHandle<i32>,
    ) {
        tokio::time::timeout(
            Duration::from_secs(25),
            wait_for_gui_build_receipt(fixture, ws, id, log, process),
        )
        .await
        .unwrap_or_else(|error| {
            panic!(
                "BES did not finalize before stdin: {error}; {}",
                String::from_utf8_lossy(&log_bytes(log))
            )
        });
    }

    async fn wait_for_gui_build_receipt(
        fixture: &GuiBazelFixture,
        ws: &Path,
        id: &str,
        log: &fs::File,
        process: &tokio::task::JoinHandle<i32>,
    ) {
        while !store::read_record(&fixture.data, ws, id)
            .is_ok_and(|record| record.exit_code == Some(0))
        {
            assert!(
                !process.is_finished(),
                "Bazel ended before recording the build: {}",
                String::from_utf8_lossy(&log_bytes(log))
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[test]
    fn pattern_validation_accepts_labels_and_patterns() {
        for pattern in [
            "//app/lib:parser",
            "//...",
            ":all",
            ":*",
            "@maven//:guava",
            "//app/...",
            // Shell metacharacters are ordinary label characters here: nothing
            // splits or evaluates the pattern, so it reaches bazel verbatim as
            // one argument.
            "//app:tar;get",
            "//a:b$(id)",
            "//a:b`id`",
            "//a:b&&c",
            "//a:b|c",
            "//a:b>c",
            // U+FEFF and U+00AD are neither White_Space nor Cc, so the core
            // takes them and the UI's `/[\p{White_Space}\p{Cc}]/u` must too.
            "//a:\u{feff}b",
            "//a:\u{ad}b",
        ] {
            assert!(validate_pattern(pattern).is_ok(), "{pattern:?}");
            assert_eq!(
                argv("build", pattern, &[], "grpc://127.0.0.1:12345", "abc",)
                    .last()
                    .unwrap(),
                pattern
            );
        }
    }

    #[test]
    fn pattern_validation_rejects_empty_whitespace_control_and_leading_dash() {
        for pattern in [
            "",
            " ",
            "//a:b ",
            "a\tb",
            "a\nb",
            "a\0b",
            "-//a:b",
            "\u{3000}",
            "//a:b; id",
            // The boundary characters the UI test pins: U+0085 and U+2028 are
            // White_Space, U+009F is Cc. `/[\p{White_Space}\p{Cc}]/u` in
            // `src/lib/target.ts` is the same rule and must stay identical.
            "\u{85}",
            "\u{9f}",
            "\u{2028}",
        ] {
            assert!(validate_pattern(pattern).is_err(), "{pattern:?}");
        }
        // One message for both rules, and it does not echo the input.
        let msg = validate_pattern("a b").unwrap_err();
        assert!(msg.contains("whitespace"), "{msg}");
        assert!(msg.contains("'-'"), "{msg}");
    }

    #[test]
    fn command_validation_accepts_build_test_and_run() {
        for cmd in ["build", "test", "run"] {
            assert!(validate_cmd(cmd).is_ok(), "{cmd:?}");
        }
        for cmd in ["clean", "query", ""] {
            assert!(validate_cmd(cmd).is_err(), "{cmd:?}");
        }
    }

    /// An unconfirmed kill revokes signal ownership; the entry stays owned
    /// until its worker releases it, then leaves the map entirely.
    #[test]
    fn unconfirmed_kill_revokes_ownership_until_the_worker_releases() {
        let core = Core::new();
        hold_process(&core, Path::new(WS), "invocation", Some(i32::MAX));
        {
            let mut st = core.state.lock().unwrap();
            begin_stop(&mut st, Path::new(WS), "invocation").unwrap();
        }
        let exit = observe_process(
            &core,
            "invocation",
            || Ok(None),
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(exit.kind(), io::ErrorKind::TimedOut);
        let st = core.state.lock().unwrap();
        assert!(held_pgid(&st, "invocation").is_none());
        assert!(is_stopping(&st, "invocation"));
        drop(st);
        release_process(&core, "invocation");
        assert!(
            !core
                .state
                .lock()
                .unwrap()
                .invocations
                .contains_key("invocation")
        );
    }

    /// A late stop for a released entry is an idempotent no-op, and a
    /// different invocation's entry is never touched.
    #[test]
    fn a_stop_after_release_never_touches_another_invocation() {
        let core = Core::new();
        hold_process(&core, Path::new(WS), "invocation", Some(i32::MAX));
        hold_process(&core, Path::new(WS), "next", Some(i32::MAX));
        {
            let mut st = core.state.lock().unwrap();
            begin_stop(&mut st, Path::new(WS), "invocation").unwrap();
        }
        let exit = observe_process(
            &core,
            "invocation",
            || Ok(None),
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap_err();
        let st = core.state.lock().unwrap();
        assert!(held_pgid(&st, "invocation").is_none());
        assert_eq!(held_pgid(&st, "next"), Some(i32::MAX));
        drop(st);
        release_process(&core, "invocation");
        {
            let mut st = core.state.lock().unwrap();
            // The released id is unknown now: a stale Stop is a no-op.
            assert!(begin_stop(&mut st, Path::new(WS), "invocation").is_ok());
            assert!(!st.invocations.contains_key("invocation"));
            assert_eq!(held_pgid(&st, "next"), Some(i32::MAX));
            assert!(!is_stopping(&st, "next"));
        }
        assert_eq!(exit.kind(), io::ErrorKind::TimedOut);
    }

    /// Best-effort SIGKILL of the test's group, so a failed assertion cannot
    /// leave the `sleep` behind.
    struct GroupGuard(i32);

    impl Drop for GroupGuard {
        fn drop(&mut self) {
            let _ = signal_group(self.0, Signal::KILL);
        }
    }

    /// Probe without delivering a signal: only ESRCH means the group is empty.
    fn group_alive(pgid: i32) -> bool {
        process::test_kill_process_group(process_group(pgid).unwrap())
            != Err(rustix::io::Errno::SRCH)
    }

    #[test]
    fn invocation_process_groups_exclude_broadcast_and_invalid_ids() {
        assert_eq!(
            process_group(0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            process_group(1).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            process_group(-1).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            process_group(i32::MIN).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(process_group(42).unwrap().as_raw_pid(), 42);
    }

    #[test]
    fn sigint_reaches_every_process_in_the_group() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30 & echo ready; wait")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id() as i32;
        let _guard = GroupGuard(pgid);

        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");

        let (tx, rx) = mpsc::channel();
        let waiter = thread::spawn(move || {
            let status = child.wait();
            let _ = tx.send(());
            status
        });
        signal_group(pgid, Signal::INT).unwrap();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("child did not exit after SIGINT");
        let status = waiter.join().unwrap().unwrap();
        assert!(!status.success(), "unexpected status: {status}");

        // A background job of a non-interactive shell may ignore SIGINT itself
        // (POSIX async list), so the group is not necessarily empty yet: kill
        // it, exactly as `stop` does after the grace, and then require every
        // member to be gone. The group may be empty already.
        let _ = signal_group(pgid, Signal::KILL);
        assert_group_dead(pgid);
    }

    /// `exit_stop` is the app-exit path, which cannot defer the exit: it must
    /// still kill groups that outlive their SIGINT, within the short budget
    /// shared by every owned invocation.
    #[test]
    fn exit_stop_kills_every_group_after_the_short_grace() {
        let mut children = Vec::new();
        let core = Core::new();
        for id in ["a", "b"] {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg("trap '' INT; sleep 30 & echo ready; wait")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap();
            let pgid = child.id() as i32;
            let mut ready = String::new();
            BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut ready)
                .unwrap();
            assert_eq!(ready.trim(), "ready");
            hold_process(&core, Path::new(WS), id, Some(pgid));
            children.push((child, pgid));
        }
        let guards: Vec<_> = children.iter().map(|(_, pgid)| GroupGuard(*pgid)).collect();

        let started = Instant::now();
        exit_stop(&core);
        // The SIGINTs did not end them, so the SIGKILLs came after the grace:
        // the wait is bounded, not skipped, and not per-invocation.
        assert!(started.elapsed() >= EXIT_GRACE, "the grace was skipped");
        assert!(
            core.state
                .lock()
                .unwrap()
                .shutting_down
                .load(Ordering::Relaxed)
        );
        for (child, pgid) in &mut children {
            let status = child.wait().unwrap();
            assert!(!status.success(), "unexpected status: {status}");
            assert_group_dead(*pgid);
        }
        drop(guards);
    }

    fn spawn_run_owner(
        core: &Arc<Core>,
        ws: &Path,
        id: &str,
        child: Child,
        capture: crate::terminal::Capture,
    ) -> thread::JoinHandle<i32> {
        let owner_core = core.clone();
        let owner_ws = ws.to_path_buf();
        let owner_id = id.to_string();
        thread::spawn(move || {
            complete_gui_run(child, capture, owner_core, &owner_ws, owner_id, None, None)
        })
    }

    /// App exit must not return while an owner thread still holds a reaped
    /// child: the shared deadline covers drain, console finish and reap.
    /// Entries leave the map on release; the fixture-owned log handles keep
    /// each run's captured bytes for the isolation check.
    #[test]
    fn exit_stop_waits_for_owner_threads_to_reap_and_drain() {
        let core = Arc::new(Core::new());
        let ws = PathBuf::from(WS);
        let mut owners = Vec::new();
        let mut logs = Vec::new();
        for (id, marker) in [("a", "marker-a"), ("b", "marker-b")] {
            let mut command = Command::new("sh");
            command
                .arg("-c")
                .arg(format!("trap '' INT; printf '{marker}\\n'; sleep 30"));
            let (child, capture, log) = spawn_gui_child(
                &core,
                &mut command,
                InvocationMeta {
                    workspace: &ws,
                    cmd: "run",
                    pattern: "//:probe",
                    id,
                },
            );
            owners.push(spawn_run_owner(&core, &ws, id, child, capture));
            logs.push((id, marker, log));
        }
        for (id, marker, _) in &logs {
            wait_for_console_marker(&core, &ws, id, marker);
        }
        let started = Instant::now();
        exit_stop(&core);
        assert!(started.elapsed() >= EXIT_GRACE, "the grace was skipped");
        assert!(core.state.lock().unwrap().invocations.is_empty());
        let entries: Vec<(&str, &str)> =
            logs.iter().map(|(id, marker, _)| (*id, *marker)).collect();
        for (position, (id, marker, log)) in logs.iter().enumerate() {
            let text = String::from_utf8(log_bytes(log)).unwrap();
            assert!(text.contains(marker), "{id}: {text}");
            assert_no_foreign_marker(id, &text, &entries, position);
        }
        for owner in owners {
            owner.join().unwrap();
        }
    }

    /// A child that never confirms leaves the shared deadline: ownership is
    /// revoked, the note explains the unknown stop, and no signal can follow.
    #[test]
    fn exit_stop_confirmation_deadline_revokes_unconfirmed_ownership() {
        let core = Core::new();
        hold_process(&core, Path::new(WS), "a", Some(i32::MAX));
        hold_process(&core, Path::new(WS), "b", Some(i32::MAX));
        let started = Instant::now();
        exit_stop(&core);
        let elapsed = started.elapsed();
        assert!(elapsed >= EXIT_GRACE + STOP_CONFIRM);
        assert!(elapsed < EXIT_GRACE + STOP_CONFIRM + Duration::from_secs(5));
        let st = core.state.lock().unwrap();
        for id in ["a", "b"] {
            assert!(held_pgid(&st, id).is_none());
            assert!(is_stopping(&st, id));
        }
    }

    #[test]
    fn owner_escalates_an_ignoring_group_and_observes_its_exit() {
        let mut child = Command::new("sh")
            .args(["-c", "trap '' INT; echo ready; while :; do sleep 1; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id() as i32;
        let _guard = GroupGuard(pgid);
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let core = Core::new();
        hold_process(&core, Path::new(WS), "invocation", Some(pgid));
        {
            let mut st = core.state.lock().unwrap();
            begin_stop(&mut st, Path::new(WS), "invocation").unwrap();
        }
        let exit = observe_process(
            &core,
            "invocation",
            || {
                child
                    .try_wait()
                    .map(|status| status.map(|status| status.code().unwrap_or(-1)))
            },
            Duration::from_millis(10),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_ne!(exit, 0);
        assert!(held_pgid(&core.state.lock().unwrap(), "invocation").is_none());
    }

    #[test]
    fn wait_errors_immediately_revoke_signal_ownership() {
        let core = Core::new();
        hold_process(&core, Path::new(WS), "invocation", Some(i32::MAX));
        let started = Instant::now();
        let error = observe_process(
            &core,
            "invocation",
            || Err(io::Error::from_raw_os_error(libc::ECHILD)),
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
        assert!(started.elapsed() < Duration::from_secs(1));
        let st = core.state.lock().unwrap();
        assert!(held_pgid(&st, "invocation").is_none());
        assert!(!is_stopping(&st, "invocation"));
        drop(st);
        release_process(&core, "invocation");
        assert!(
            !core
                .state
                .lock()
                .unwrap()
                .invocations
                .contains_key("invocation")
        );
    }

    #[test]
    fn reap_revokes_signal_ownership_before_the_process_slot_is_released() {
        let core = Core::new();
        hold_process(&core, Path::new(WS), "invocation", Some(i32::MAX));
        assert_eq!(
            observe_process(
                &core,
                "invocation",
                || Ok(Some(7)),
                STOP_GRACE,
                STOP_CONFIRM
            )
            .unwrap(),
            7
        );
        let mut st = core.state.lock().unwrap();
        assert!(begin_stop(&mut st, Path::new(WS), "invocation").is_ok());
        assert!(held_pgid(&st, "invocation").is_none());
        assert!(!is_stopping(&st, "invocation"));
    }

    fn spawn_pipes(core: &Core, ws: &Path, id: &str, command: &mut Command) -> Child {
        let mut st = core.state.lock().unwrap();
        register_ws(&mut st, ws);
        let (child, capture) = spawn(
            &mut st,
            command,
            InvocationMeta {
                workspace: ws,
                cmd: "query",
                pattern: "//pkg:*",
                id,
            },
            Output::Pipes,
        )
        .unwrap();
        assert!(capture.is_none());
        child
    }

    fn wait_child(core: &Core, id: &str, child: &mut Child) -> i32 {
        wait(core, id, || {
            child
                .try_wait()
                .map(|status| status.map(|status| status.code().unwrap_or(-1)))
        })
        .unwrap()
    }

    fn assert_three_owned_processes(st: &CoreState) {
        assert_eq!(process_count(st), 3);
        for id in ["a1", "a2", "b1"] {
            assert!(st.invocations.contains_key(id), "{id} missing");
        }
    }

    #[test]
    fn two_invocations_in_one_workspace_and_two_workspaces_are_all_accepted() {
        let core = Core::new();
        let ws_a = PathBuf::from("/ws/a");
        let ws_b = PathBuf::from("/ws/b");
        let mut children = Vec::new();
        for (ws, id) in [(&ws_a, "a1"), (&ws_a, "a2"), (&ws_b, "b1")] {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 0"]);
            children.push((spawn_pipes(&core, ws, id, &mut command), id));
        }
        assert_three_owned_processes(&core.state.lock().unwrap());
        for (mut child, id) in children {
            assert_eq!(wait_child(&core, id, &mut child), 0);
            release_process(&core, id);
            assert!(!core.state.lock().unwrap().invocations.contains_key(id));
        }
    }

    #[test]
    fn finishing_one_invocation_leaves_another_untouched() {
        let core = Core::new();
        let ws = Path::new(WS);
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 60"]);
        let mut child_a = spawn_pipes(&core, ws, "a", &mut command);
        let guard_a = GroupGuard(child_a.id() as i32);
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 60"]);
        let mut child_b = spawn_pipes(&core, ws, "b", &mut command);
        let guard_b = GroupGuard(child_b.id() as i32);

        // Stop A: B's entry, group and console-less process stay live.
        begin_stop(&mut core.state.lock().unwrap(), ws, "a").unwrap();
        assert_eq!(wait_child(&core, "a", &mut child_a), -1);
        release_process(&core, "a");
        {
            let st = core.state.lock().unwrap();
            let b = &st.invocations["b"];
            assert_eq!(b.process.pgid, Some(child_b.id() as i32));
            assert!(!is_stopping(&st, "b"));
        }
        // A repeated stop for A is an idempotent no-op now that its entry
        // is gone; it never fails and never signals.
        assert!(begin_stop(&mut core.state.lock().unwrap(), ws, "a").is_ok());
        // B's real child is still alive before its own stop.
        assert!(
            child_b.try_wait().unwrap().is_none(),
            "stopping A terminated B"
        );
        // B still waits and can be stopped independently.
        begin_stop(&mut core.state.lock().unwrap(), ws, "b").unwrap();
        assert_eq!(wait_child(&core, "b", &mut child_b), -1);
        drop(guard_a);
        drop(guard_b);
    }

    #[test]
    fn stop_rejects_only_a_foreign_workspace() {
        let core = Core::new();
        hold_process(&core, Path::new("/ws/a"), "a", Some(i32::MAX));
        {
            let mut st = core.state.lock().unwrap();
            // A missing id is an idempotent no-op: a stale Stop must not fail.
            assert!(begin_stop(&mut st, Path::new("/ws/a"), "missing").is_ok());
            // A live id owned by another workspace is still rejected.
            assert!(begin_stop(&mut st, Path::new("/ws/b"), "a").is_err());
            assert!(!is_stopping(&st, "a"));
            assert_eq!(held_pgid(&st, "a"), Some(i32::MAX));
        }
    }

    #[test]
    fn repeated_stop_does_not_extend_deadlines() {
        let core = Core::new();
        let ws = Path::new(WS);
        hold_process(&core, ws, "invocation", Some(i32::MAX));
        hold_process(&core, ws, "other", Some(i32::MAX));
        let mut st = core.state.lock().unwrap();
        begin_stop(&mut st, ws, "invocation").unwrap();
        let first = st.invocations["invocation"].process.stop_started;
        begin_stop(&mut st, ws, "invocation").unwrap();
        assert_eq!(st.invocations["invocation"].process.stop_started, first);
        assert!(st.invocations["other"].process.stop_started.is_none());
    }

    #[test]
    fn wait_query_reaps_the_child_but_keeps_the_slot_until_the_guard_drops() {
        let core = Core::new();
        let ws = Path::new(WS);
        hold_process(&core, ws, "other", Some(i32::MAX));
        let mut command = Command::new("sh");
        command.args(["-c", "exit 7"]);
        let mut child = spawn_pipes(&core, ws, "query", &mut command);
        assert_eq!(wait_query(&core, "query", &mut child).unwrap(), 7);
        {
            let st = core.state.lock().unwrap();
            let invocation = &st.invocations["query"];
            assert!(invocation.process.pgid.is_none());
            assert_eq!(held_pgid(&st, "other"), Some(i32::MAX));
        }
        release_process(&core, "query");
        let st = core.state.lock().unwrap();
        assert!(!st.invocations.contains_key("query"));
        assert!(st.invocations.contains_key("other"));
    }

    #[test]
    fn wait_query_reports_a_stopped_query() {
        let core = Core::new();
        let ws = Path::new(WS);
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 60"]);
        let mut child = spawn_pipes(&core, ws, "query", &mut command);
        let guard = GroupGuard(child.id() as i32);
        begin_stop(&mut core.state.lock().unwrap(), ws, "query").unwrap();
        assert_eq!(
            wait_query(&core, "query", &mut child).unwrap_err(),
            "Query stopped"
        );
        {
            let st = core.state.lock().unwrap();
            let process = &st.invocations["query"].process;
            assert!(process.pgid.is_none());
            assert!(process.stop_started.is_some());
        }
        release_process(&core, "query");
        assert!(!core.state.lock().unwrap().invocations.contains_key("query"));
        drop(guard);
    }

    #[test]
    fn old_owner_never_polls_or_signals_another_invocation() {
        let core = Core::new();
        hold_process(&core, Path::new(WS), "next", Some(i32::MAX));
        assert!(
            observe_process(
                &core,
                "old",
                || panic!("old owner polled"),
                Duration::ZERO,
                Duration::ZERO
            )
            .is_err()
        );
        let st = core.state.lock().unwrap();
        assert!(st.invocations.contains_key("next"));
        assert_eq!(held_pgid(&st, "next"), Some(i32::MAX));
    }

    #[test]
    fn panic_releases_the_process_slot_and_wakes_exit() {
        let core = Arc::new(Core::new());
        hold_invocation(
            &core,
            Path::new(WS),
            "invocation",
            crate::terminal::Control::stub(),
            i32::MAX,
            crate::terminal::session_log().unwrap(),
        );
        let exiting_core = core.clone();
        let exiting = thread::spawn(move || exit_stop(&exiting_core));
        let owning_core = core.clone();
        let owner = thread::spawn(move || {
            let _guard = InvocationGuard {
                core: &owning_core,
                id: "invocation",
                emit: || (),
            };
            panic!("injected save panic");
        });
        assert!(owner.join().is_err());
        exiting.join().unwrap();
        let st = core.state.lock().unwrap();
        // Even on panic the guard removes the whole entry: process and
        // console leave the map together.
        assert!(!st.invocations.contains_key("invocation"));
    }

    #[test]
    fn a_stop_racing_with_reap_cannot_signal_the_reaped_pid() {
        let core = Arc::new(Core::new());
        hold_process(&core, Path::new(WS), "invocation", Some(i32::MAX));
        let (polling_tx, polling_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let owner_core = core.clone();
        let poll = move || {
            polling_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(Some(0))
        };
        let owner = thread::spawn(move || {
            observe_process(&owner_core, "invocation", poll, STOP_GRACE, STOP_CONFIRM)
        });
        polling_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (stopping_tx, stopping_rx) = mpsc::sync_channel(1);
        let stopping_core = core.clone();
        let stopping = thread::spawn(move || {
            stopping_tx.send(()).unwrap();
            // The reap already revoked the group: the late stop is a no-op
            // and still never signals a possibly recycled pid.
            begin_stop(
                &mut stopping_core.state.lock().unwrap(),
                Path::new(WS),
                "invocation",
            )
        });
        stopping_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(owner.join().unwrap().unwrap(), 0);
        assert!(stopping.join().unwrap().is_ok());
        let st = core.state.lock().unwrap();
        assert!(held_pgid(&st, "invocation").is_none());
        assert!(!is_stopping(&st, "invocation"));
    }

    #[test]
    fn app_exit_waits_for_a_reaped_invocations_save_and_never_signals_it() {
        let core = Arc::new(Core::new());
        hold_process(&core, Path::new(WS), "invocation", None);
        let (finish_tx, finish_rx) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let finishing_core = core.clone();
        let finishing = thread::spawn(move || {
            let _guard = InvocationGuard {
                core: &finishing_core,
                id: "invocation",
                emit: || (),
            };
            ready_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (exited_tx, exited_rx) = mpsc::sync_channel(1);
        let exiting_core = core.clone();
        let exiting = thread::spawn(move || {
            exit_stop(&exiting_core);
            exited_tx.send(()).unwrap();
        });
        assert!(matches!(
            exited_rx.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        finish_tx.send(()).unwrap();
        exited_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        finishing.join().unwrap();
        exiting.join().unwrap();
        let st = core.state.lock().unwrap();
        assert!(!st.invocations.contains_key("invocation"));
    }

    #[test]
    fn exit_rejects_new_starts_once_shutdown_begins() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.shutting_down.store(true, Ordering::Relaxed);
        register_ws(&mut st, Path::new(WS));
        st.bazel = Some(PathBuf::from("/bin/true"));
        st.bes_endpoint = Some("grpc://127.0.0.1:1".into());
        let error = spawn(
            &mut st,
            &mut Command::new("true"),
            InvocationMeta {
                workspace: Path::new(WS),
                cmd: "build",
                pattern: "//:b",
                id: "late",
            },
            Output::Pipes,
        )
        .err()
        .expect("shutdown must reject the start");
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(!st.invocations.contains_key("late"));
    }
}
