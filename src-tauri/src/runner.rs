//! Runs build/test/run under a PTY; ad-hoc queries use stdout/stderr pipes.
//! Terminal output is saved independently of the Console view.
//! Build history is received by the app-owned BES, independently of process exit.
//!
//! Stop for an owned invocation uses its process group: SIGINT, then SIGKILL
//! after the shared grace period. An unknown exit releases the process slot.
//! Only the invocation's own entry may be signalled; descendants are not
//! supervised.

use crate::runargs::{argv, parse_arguments, validate_cmd, validate_pattern};
use crate::state::{ActiveInvocation, Core, CoreState, OwnedInvocation, now_millis};
use rustix::process::{self, Pid, Signal};
#[cfg(test)]
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Time SIGINT gets to end the invocation before its group is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// Time SIGKILL gets to end the invocation before its exit is called unknown
/// and its process slot is released.
const STOP_CONFIRM: Duration = Duration::from_secs(2);
/// App exit gives live children at most `EXIT_GRACE` of polling before
/// SIGKILL, shared across every owned invocation. A reaped child's in-progress
/// save is joined separately.
const EXIT_GRACE: Duration = Duration::from_secs(1);
const EXIT_POLL: Duration = Duration::from_millis(25);

/// Shown when the group was killed but no exit was confirmed.
const STOP_UNCONFIRMED: &str = "Could not confirm the stop";

/// Shown when Bazel exits 9 on build/test: another command holds the
/// output_base lock. `run` is excluded because its program may exit 9 itself.
const BUSY_NOTE: &str =
    "Bazel is busy (output_base lock held); retry when the other command finishes";

pub(crate) enum Output<'a> {
    Terminal(&'a std::fs::File),
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
    output: Output<'_>,
) -> io::Result<(Child, Option<crate::terminal::Capture>)> {
    if state.shutting_down {
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
    let (child, terminal, capture) = match output {
        Output::Terminal(log) => {
            let (child, terminal, capture) = crate::terminal::spawn(command, log)?;
            (child, Some(terminal), Some(capture))
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
        ),
    };
    state.invocations.insert(
        meta.id.to_string(),
        OwnedInvocation {
            workspace: meta.workspace.to_path_buf(),
            cmd: meta.cmd.to_string(),
            pattern: meta.pattern.to_string(),
            started_at: now_millis(),
            process: Some(ActiveInvocation {
                terminal,
                pgid: Some(child.id() as i32),
                stop_started: None,
            }),
            console: None,
            process_exit_code: None,
            note: None,
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
        Output::Terminal(&log),
    )
    .map_err(|e| format!("failed to spawn {}: {e}", bazel.display()))?;
    let capture = capture.expect("terminal output capture");
    st.invocations
        .get_mut(&id)
        .expect("just registered")
        .console = Some(log);
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
        let exit = wait(&core, &id, || poll_capture(&mut child, &capture));
        // Reap already revoked signal ownership. Keep the entry until terminal
        // capture ends; build history is finalized only by BES.
        let captured = capture.finish();
        let mut st = core.state.lock().expect("application state lock poisoned");
        if apply_reap(&mut st, &id, &exit)
            && let Err(error) = captured
            && let Some(invocation) = st.invocations.get_mut(&id)
        {
            invocation.note = Some(format!("Failed to save the terminal output: {error}"));
        }
    });
    Ok(result)
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

/// Signal the invocation's group once. A repeated stop for the same entry is
/// a no-op; finished, reaped or foreign entries are rejected.
fn begin_stop(st: &mut CoreState, ws: &Path, id: &str) -> Result<(), String> {
    let invocation = st
        .invocations
        .get_mut(id)
        .ok_or_else(|| "no such invocation".to_string())?;
    if invocation.workspace != ws {
        return Err("invocation belongs to another workspace".to_string());
    }
    let process = invocation
        .process
        .as_mut()
        .ok_or_else(|| "invocation already finished".to_string())?;
    if process.stop_started.is_some() {
        return Ok(());
    }
    let Some(pgid) = process.pgid else {
        return Err("invocation is not running".to_string());
    };
    process.stop_started = Some(Instant::now());
    invocation.note = None;
    if let Err(e) = signal_process(process, pgid, Signal::INT) {
        log::warn!("SIGINT to invocation group failed: {e}");
    }
    Ok(())
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
    let mut st = core.state.lock().expect("application state lock poisoned");
    let stopping = st
        .invocations
        .get(id)
        .and_then(|invocation| invocation.process.as_ref())
        .is_some_and(|process| process.stop_started.is_some());
    apply_reap(&mut st, id, &waited);
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
            .and_then(|invocation| invocation.process.as_mut())
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
/// The owning worker finished: release the process slot. A console-less
/// entry leaves nothing behind; a console keeps its entry readable.
fn release_invocation(st: &mut CoreState, id: &str) {
    let Some(invocation) = st.invocations.get_mut(id) else {
        return;
    };
    let owned = invocation.process.take().is_some();
    if owned && thread::panicking() {
        invocation.note = Some("Failed to finish processing the invocation result".into());
    }
    if invocation.console.is_none() {
        st.invocations.remove(id);
    }
}

impl<F: Fn()> Drop for InvocationGuard<'_, F> {
    fn drop(&mut self) {
        let mut st = self
            .core
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        release_invocation(&mut st, self.id);
        self.core.invocation_finished.notify_all();
        drop(st);
        // Avoid a second panic from an event emission while unwinding.
        if !thread::panicking() {
            (self.emit)();
        }
    }
}

/// Apply the reap of `id` to its entry and return whether this reap belongs
/// to a process still owned there. A `wait` error leaves the exit unknown,
/// with the reason in the note. Other invocations are never touched.
fn apply_reap(st: &mut CoreState, id: &str, exit: &io::Result<i32>) -> bool {
    let Some(invocation) = st.invocations.get_mut(id) else {
        return false;
    };
    let Some(process) = invocation.process.as_mut() else {
        return false;
    };
    process.pgid = None;
    invocation.process_exit_code = exit.as_ref().ok().copied();
    invocation.note = match exit {
        Ok(9)
            if matches!(
                invocation.cmd.as_str(),
                "build" | "test" | "query" | "cquery" | "aquery"
            ) =>
        {
            Some(BUSY_NOTE.to_string())
        }
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::TimedOut => Some(STOP_UNCONFIRMED.to_string()),
        Err(error) => Some(format!("Could not confirm the exit: {error}")),
    };
    true
}

fn kill_invocation_groups(st: &CoreState) {
    for invocation in st.invocations.values() {
        let Some(process) = invocation.process.as_ref() else {
            continue;
        };
        let Some(pgid) = process.pgid else {
            continue;
        };
        if let Err(error) = signal_process(process, pgid, Signal::KILL) {
            log::warn!("SIGKILL to invocation group failed: {error}");
        }
    }
}

fn revoke_unconfirmed_groups(st: &mut CoreState) {
    for invocation in st.invocations.values_mut() {
        if let Some(process) = invocation.process.as_mut()
            && process.pgid.take().is_some()
        {
            invocation.note = Some(STOP_UNCONFIRMED.to_string());
        }
    }
}

fn interrupt_invocations(st: &mut CoreState) {
    st.shutting_down = true;
    for invocation in st.invocations.values_mut() {
        let Some(process) = invocation.process.as_mut() else {
            continue;
        };
        let Some(pgid) = process.pgid.filter(|_| process.stop_started.is_none()) else {
            continue;
        };
        process.stop_started = Some(Instant::now());
        invocation.note = None;
        if let Err(e) = signal_process(process, pgid, Signal::INT) {
            log::warn!("SIGINT to invocation group failed: {e}");
        }
    }
}

/// Bound process stopping to the short exit grace, shared across every owned
/// invocation: one common deadline, not one per child. Children already
/// reaped have nothing left to signal; their in-progress saves are joined
/// through the condvar. New starts are rejected by `shutting_down`.
pub fn exit_stop(core: &Core) {
    let owned = {
        let mut st = core.state.lock().expect("application state lock poisoned");
        interrupt_invocations(&mut st);
        st.invocations.values().any(|entry| entry.process.is_some())
    };
    if !owned {
        return;
    }
    let kill_at = Instant::now() + EXIT_GRACE;
    let finish_at = kill_at + STOP_CONFIRM;
    let mut killed = false;
    loop {
        let mut st = core.state.lock().expect("application state lock poisoned");
        if !st.invocations.values().any(|entry| entry.process.is_some()) {
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
    use crate::state::{OwnedInvocation, WorkspaceState};
    use crate::store;
    use std::path::{Path, PathBuf};

    const WS: &str = "/ws";

    fn register_ws(st: &mut CoreState, ws: &Path) {
        st.workspaces
            .entry(ws.to_path_buf())
            .or_insert_with(|| WorkspaceState::new(crate::scope::Scope::default()));
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
                process: Some(ActiveInvocation {
                    terminal: Some(terminal),
                    pgid: Some(pgid),
                    stop_started: None,
                }),
                console: Some(log),
                process_exit_code: None,
                note: None,
            },
        );
    }

    /// Finish what a worker's guard drop does: release the process slot after
    /// `apply_reap` already revoked signal ownership.
    fn release_process(core: &Core, id: &str) {
        drop(InvocationGuard {
            core,
            id,
            emit: || (),
        });
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
        let id = meta.id.to_string();
        let (child, capture) =
            spawn(&mut st, command, meta, Output::Terminal(&log)).expect("gui child spawn");
        st.invocations.get_mut(&id).unwrap().console = Some(log);
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

    fn assert_drained(core: &Core, ids: &[&str]) {
        let st = core.state.lock().unwrap();
        for id in ids {
            let invocation = &st.invocations[*id];
            assert!(invocation.process.is_none(), "{id} still owns a process");
            assert!(invocation.console.is_some(), "{id} console lost");
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

    fn assert_console_isolated(core: &Core, ws: &Path, entries: &[(&str, &str)]) {
        for (index, (id, marker)) in entries.iter().enumerate() {
            let text = console_text(core, ws, id);
            assert!(text.contains(marker), "{id}: {text}");
            assert_no_foreign_marker(id, &text, entries, index);
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
                process: Some(ActiveInvocation {
                    terminal: None,
                    pgid,
                    stop_started: None,
                }),
                console: None,
                process_exit_code: None,
                note: None,
            },
        );
    }

    fn held_pgid(st: &CoreState, id: &str) -> Option<i32> {
        st.invocations
            .get(id)
            .and_then(|invocation| invocation.process.as_ref())
            .and_then(|process| process.pgid)
    }

    fn is_stopping(st: &CoreState, id: &str) -> bool {
        st.invocations
            .get(id)
            .and_then(|invocation| invocation.process.as_ref())
            .is_some_and(|process| process.stop_started.is_some())
    }

    fn process_count(st: &CoreState) -> usize {
        st.invocations
            .values()
            .filter(|invocation| invocation.process.is_some())
            .count()
    }

    use std::io::{BufRead, BufReader};
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

    #[test]
    fn failed_process_before_started_keeps_stderr_in_the_completed_console() {
        let temp = LogDir::new();
        let ws = temp.0.join("workspace");
        fs::create_dir(&ws).unwrap();
        let ws = ws.canonicalize().unwrap();
        let data = temp.0.join("data");
        let id = uuid::Uuid::new_v4().to_string();
        let log = crate::terminal::session_log().unwrap();
        let (mut child, terminal, mut capture) = crate::terminal::spawn(
            Command::new("sh").args(["-c", "printf 'startup failed\\n' >&2; exit 7"]),
            &log,
        )
        .unwrap();
        let core = Core::new();
        hold_invocation(&core, &ws, &id, terminal, child.id() as i32, log);
        let exit = Ok(child.wait().unwrap().code().unwrap());
        capture.finish().unwrap();
        {
            let mut state = core.state.lock().unwrap();
            assert!(apply_reap(&mut state, &id, &exit));
            assert!(begin_stop(&mut state, &ws, &id).is_err());
        }
        release_process(&core, &id);
        assert!(store::read_record(&data, &ws, &id).is_err());
        assert!(crate::terminal::resize(&core, ws.to_str().unwrap(), &id, 80, 24).is_ok());
        let console = serde_json::to_value(
            crate::console::read(&core, ws.to_str().unwrap(), &id, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(console["done"], true);
        let bytes: Vec<u8> = serde_json::from_value(console["data"].clone()).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "startup failed\r\n");
    }

    /// A stale worker can never mark another invocation's console finished:
    /// completion is the entry's own released process slot.
    #[test]
    fn console_completion_follows_the_entries_own_process_slot() {
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
                    process: Some(ActiveInvocation::stub(Some(42))),
                    console: Some(crate::terminal::session_log().unwrap()),
                    process_exit_code: None,
                    note: None,
                },
            );
            // A reaped but still-draining process is not a finished console.
            state
                .invocations
                .get_mut("new")
                .unwrap()
                .process
                .as_mut()
                .unwrap()
                .pgid = None;
        }
        let output =
            serde_json::to_value(crate::console::read(&core, WS, "new", 0).unwrap()).unwrap();
        assert_eq!(output["done"], false);
        release_process(&core, "new");
        let output =
            serde_json::to_value(crate::console::read(&core, WS, "new", 0).unwrap()).unwrap();
        assert_eq!(output["done"], true);
        // A foreign id's guard cannot release "new"'s slot.
        release_process(&core, "old");
        let st = core.state.lock().unwrap();
        assert!(st.invocations["new"].process.is_none());
        assert!(st.invocations["new"].console.is_some());
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
        assert_eq!(store::list(&fixture.data, &fixture.ws).unwrap().len(), 3);
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
        let process = tokio::task::spawn_blocking(move || {
            complete_gui_run(
                child,
                capture,
                owner_core,
                &owner_ws,
                owner_id,
                (cmd == "run").then_some(input),
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
            core.state
                .lock()
                .unwrap()
                .invocations
                .get(&id)
                .is_some_and(|invocation| invocation.process.is_none())
        );
        assert_saved_console(&core, &fixture.ws, &id, &output);
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
        assert!(store::artifact(&fixture.data, &fixture.ws, id, "bep.binpb.gz").is_ok());
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
        let process = tokio::task::spawn_blocking(move || {
            complete_gui_run(
                child,
                capture,
                owner_core,
                &owner_ws,
                owner_id,
                gated.then_some(released),
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
                invocation
                    .process
                    .as_ref()
                    .is_some_and(|process| process.pgid.is_some()),
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
            invocation
                .process
                .as_ref()
                .is_some_and(|process| process.pgid.is_some()),
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
        let text_a = console_text(&core, &runs[0].ws, &runs[0].id);
        let text_b = console_text(&core, &runs[1].ws, &runs[1].id);
        assert!(text_b.contains("42\r\n"), "B console: {text_b}");
        assert!(!text_a.contains("42\r\n"), "A console: {text_a}");
        let ids: Vec<&str> = runs.iter().map(|run| run.id.as_str()).collect();
        assert_drained(&core, &ids);
    }

    /// Completed consoles expose captured bytes even when reopened, but reject new input.
    fn assert_saved_console(core: &Core, ws: &Path, id: &str, output: &str) {
        let console =
            serde_json::to_value(crate::console::read(core, ws.to_str().unwrap(), id, 0).unwrap())
                .unwrap();
        assert_eq!(
            console["done"], true,
            "final terminal bytes must already be saved"
        );
        let bytes: Vec<u8> = serde_json::from_value(console["data"].clone()).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), output);
        let reopened =
            serde_json::to_value(crate::console::read(core, ws.to_str().unwrap(), id, 0).unwrap())
                .unwrap();
        assert_eq!(reopened, console);
        assert!(crate::terminal::write(core, ws.to_str().unwrap(), id, b"late input").is_err());
    }

    fn complete_gui_run(
        mut child: std::process::Child,
        mut capture: crate::terminal::Capture,
        core: Arc<Core>,
        ws: &Path,
        id: String,
        input: Option<mpsc::Receiver<()>>,
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
        let exit = observe_process(
            &core,
            &id,
            || Ok(child.try_wait()?.map(|status| status.code().unwrap_or(-1))),
            STOP_GRACE,
            STOP_CONFIRM,
        );
        capture.finish().unwrap();
        {
            let mut state = core.state.lock().unwrap();
            apply_reap(&mut state, &id, &exit);
        }
        release_process(&core, &id);
        exit.unwrap()
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

    /// An unconfirmed kill releases the entry's process slot with the note
    /// that says the exit is unknown; its console stays readable.
    #[test]
    fn unconfirmed_kill_releases_the_process_with_the_unknown_exit_note() {
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
        let mut st = core.state.lock().unwrap();
        assert!(held_pgid(&st, "invocation").is_none());
        assert!(apply_reap(&mut st, "invocation", &Err(exit)));
        assert_eq!(
            st.invocations["invocation"].note.as_deref(),
            Some(STOP_UNCONFIRMED)
        );
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

    /// A reap after the stop gave up writes nothing, and a different
    /// invocation's entry is never touched.
    #[test]
    fn an_expired_reap_never_releases_another_invocations_process() {
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
        let mut st = core.state.lock().unwrap();
        assert!(apply_reap(&mut st, "invocation", &Err(exit)));
        assert_eq!(
            st.invocations["invocation"].note.as_deref(),
            Some(STOP_UNCONFIRMED)
        );
        assert!(st.invocations["next"].process.is_some());
        assert_eq!(held_pgid(&st, "next"), Some(i32::MAX));
        assert!(st.invocations["next"].note.is_none());
        drop(st);
        release_process(&core, "invocation");
        let mut st = core.state.lock().unwrap();
        assert!(!apply_reap(
            &mut st,
            "invocation",
            &Err(io::Error::other("a second reap"))
        ));
        assert!(!st.invocations.contains_key("invocation"));
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
        assert!(core.state.lock().unwrap().shutting_down);
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
            complete_gui_run(child, capture, owner_core, &owner_ws, owner_id, None)
        })
    }

    /// App exit must not return while an owner thread still holds a reaped
    /// child: the shared deadline covers drain, console finish and reap.
    #[test]
    fn exit_stop_waits_for_owner_threads_to_reap_and_drain() {
        let core = Arc::new(Core::new());
        let ws = PathBuf::from(WS);
        let mut owners = Vec::new();
        for (id, marker) in [("a", "marker-a"), ("b", "marker-b")] {
            let mut command = Command::new("sh");
            command
                .arg("-c")
                .arg(format!("trap '' INT; printf '{marker}\\n'; sleep 30"));
            let (child, capture, _log) = spawn_gui_child(
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
        }
        for (id, marker) in [("a", "marker-a"), ("b", "marker-b")] {
            wait_for_console_marker(&core, &ws, id, marker);
        }
        let started = Instant::now();
        exit_stop(&core);
        assert!(started.elapsed() >= EXIT_GRACE, "the grace was skipped");
        assert_drained(&core, &["a", "b"]);
        assert_console_isolated(&core, &ws, &[("a", "marker-a"), ("b", "marker-b")]);
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
            assert_eq!(st.invocations[id].note.as_deref(), Some(STOP_UNCONFIRMED));
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
        let mut st = core.state.lock().unwrap();
        assert!(held_pgid(&st, "invocation").is_none());
        assert!(!is_stopping(&st, "invocation"));
        assert!(apply_reap(&mut st, "invocation", &Err(error)));
        assert!(
            st.invocations["invocation"]
                .note
                .as_ref()
                .unwrap()
                .contains("Could not confirm the exit")
        );
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
        assert!(begin_stop(&mut st, Path::new(WS), "invocation").is_err());
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

    fn kill_and_reap(core: &Core, mut child: Child, id: &str) {
        let _guard = GroupGuard(child.id() as i32);
        child.kill().unwrap();
        assert_eq!(wait_child(core, id, &mut child), -1);
        {
            let mut st = core.state.lock().unwrap();
            apply_reap(&mut st, id, &Ok(-1));
        }
        release_process(core, id);
        assert!(!core.state.lock().unwrap().invocations.contains_key(id));
    }

    #[test]
    fn two_invocations_in_one_workspace_and_two_workspaces_are_all_accepted() {
        let core = Core::new();
        let ws_a = PathBuf::from("/ws/a");
        let ws_b = PathBuf::from("/ws/b");
        let mut children = Vec::new();
        for (ws, id) in [(&ws_a, "a1"), (&ws_a, "a2"), (&ws_b, "b1")] {
            let mut command = Command::new("sh");
            command.args(["-c", "exec sleep 60"]);
            children.push((spawn_pipes(&core, ws, id, &mut command), id));
        }
        assert_three_owned_processes(&core.state.lock().unwrap());
        for (child, id) in children {
            kill_and_reap(&core, child, id);
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
        {
            let mut st = core.state.lock().unwrap();
            let _ = apply_reap(&mut st, "a", &Ok(-1));
        }
        release_process(&core, "a");
        {
            let st = core.state.lock().unwrap();
            let b = &st.invocations["b"];
            assert!(b.process.is_some());
            assert_eq!(b.process.as_ref().unwrap().pgid, Some(child_b.id() as i32));
            assert!(!is_stopping(&st, "b"));
        }
        // A repeated stop for A is rejected now that its entry is gone.
        assert!(begin_stop(&mut core.state.lock().unwrap(), ws, "a").is_err());
        // B still waits and can be stopped independently.
        begin_stop(&mut core.state.lock().unwrap(), ws, "b").unwrap();
        assert_eq!(wait_child(&core, "b", &mut child_b), -1);
        drop(guard_a);
        drop(guard_b);
    }

    #[test]
    fn stop_rejects_unknown_ids_and_workspace_mismatch() {
        let core = Core::new();
        hold_process(&core, Path::new("/ws/a"), "a", Some(i32::MAX));
        {
            let mut st = core.state.lock().unwrap();
            assert!(begin_stop(&mut st, Path::new("/ws/a"), "missing").is_err());
            assert!(begin_stop(&mut st, Path::new("/ws/b"), "a").is_err());
            // The failed attempts left the entry unstopped.
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
        let first = st.invocations["invocation"]
            .process
            .as_ref()
            .and_then(|process| process.stop_started);
        begin_stop(&mut st, ws, "invocation").unwrap();
        assert_eq!(
            st.invocations["invocation"]
                .process
                .as_ref()
                .and_then(|process| process.stop_started),
            first
        );
        assert!(
            st.invocations["other"]
                .process
                .as_ref()
                .is_some_and(|process| process.stop_started.is_none())
        );
    }

    /// A console-less entry is pruned only when its worker's guard drops:
    /// reap alone keeps the exit and note for the in-flight drain.
    #[test]
    fn a_console_less_entry_is_pruned_only_when_its_worker_releases_it() {
        let core = Core::new();
        let ws = Path::new(WS);
        hold_process(&core, ws, "query", None);
        {
            let mut st = core.state.lock().unwrap();
            assert!(apply_reap(&mut st, "query", &Ok(0)));
            let invocation = &st.invocations["query"];
            assert!(invocation.process.is_some());
            assert_eq!(invocation.process_exit_code, Some(0));
        }
        release_process(&core, "query");
        assert!(!core.state.lock().unwrap().invocations.contains_key("query"));
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
            assert!(invocation.process.is_some());
            assert!(invocation.process.as_ref().unwrap().pgid.is_none());
            assert_eq!(invocation.process_exit_code, Some(7));
            assert_eq!(held_pgid(&st, "other"), Some(i32::MAX));
        }
        release_process(&core, "query");
        let st = core.state.lock().unwrap();
        assert!(!st.invocations.contains_key("query"));
        assert!(st.invocations["other"].process.is_some());
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
            let process = st.invocations["query"].process.as_ref().unwrap();
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
        assert!(st.invocations["next"].process.is_some());
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
        let invocation = &st.invocations["invocation"];
        assert!(invocation.process.is_none());
        assert!(invocation.console.is_some());
        assert!(
            invocation
                .note
                .as_ref()
                .unwrap()
                .contains("finish processing")
        );
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
            // The reap already revoked the group: the late stop must fail
            // without signalling a possibly recycled pid.
            let _ = begin_stop(
                &mut stopping_core.state.lock().unwrap(),
                Path::new(WS),
                "invocation",
            );
        });
        stopping_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(owner.join().unwrap().unwrap(), 0);
        stopping.join().unwrap();
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
        st.shutting_down = true;
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

    #[test]
    fn exit_9_is_a_lock_note_for_build_test_and_queries_but_never_for_run() {
        for (cmd, expect_note) in [
            ("build", true),
            ("test", true),
            ("query", true),
            ("cquery", true),
            ("aquery", true),
            ("run", false),
        ] {
            let core = Core::new();
            hold_process(&core, Path::new(WS), "invocation", None);
            let mut st = core.state.lock().unwrap();
            st.invocations.get_mut("invocation").unwrap().cmd = cmd.to_string();
            assert!(apply_reap(&mut st, "invocation", &Ok(9)));
            let invocation = &st.invocations["invocation"];
            assert_eq!(invocation.note.is_some(), expect_note, "{cmd}");
            assert_eq!(
                invocation.process_exit_code,
                Some(9),
                "{cmd}: the process exit is kept, never a BEP failure"
            );
        }
    }
}
