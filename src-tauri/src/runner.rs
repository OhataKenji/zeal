//! Runs `bazel build|test|run <target>` and captures terminal.log.
//! All commands use a PTY; output is saved independently of the Console view.
//! Build history is received by the app-owned BES, independently of process exit.
//!
//! Stop is best-effort: SIGINT, then SIGKILL after a grace period. An unknown
//! exit releases the slot with a note. Only
//! the currently held run may be signalled; descendants are not supervised.

use crate::runargs::{argv, parse_arguments, validate_cmd, validate_target};
use crate::state::{ActiveRun, ConsoleSession, Core, CoreState};
use crate::store::{self, RunRecord};
use rustix::process::{self, Pid, Signal};
#[cfg(test)]
use std::fs;
use std::io;
#[cfg(test)]
use std::os::unix::process::CommandExt;
use std::path::Path;
#[cfg(test)]
use std::process::Stdio;
use std::process::{Child, Command};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Time SIGINT gets to end the run before its group is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// Time SIGKILL gets to end the run before its exit is called unknown and its
/// slot is released.
const STOP_CONFIRM: Duration = Duration::from_secs(2);
/// App exit gives a live child at most `EXIT_GRACE` of polling before SIGKILL.
/// A reaped child's in-progress save is joined separately.
const EXIT_GRACE: Duration = Duration::from_secs(1);
const EXIT_POLL: Duration = Duration::from_millis(25);

/// Shown when the group was killed but no exit was confirmed.
const STOP_UNCONFIRMED: &str = "Could not confirm the stop";

/// Start a run in the open workspace and return its record. The run finishes
/// in the background; target data is refreshed only on request.
pub fn start(
    app: AppHandle,
    core: Arc<Core>,
    data: &Path,
    cmd: &str,
    target: &str,
    arguments: &str,
) -> Result<RunRecord, String> {
    let LaunchedRun {
        record: rec,
        mut child,
        capture,
    } = launch(&core, data, cmd, target, arguments)?;
    core.emit(&app);

    let id = rec.id.clone();
    thread::spawn(move || {
        let _slot = RunSlot {
            core: &core,
            id: &id,
            emit: || core.emit(&app),
        };
        // Drop capture before the slot on unwind, so Console completion can
        // never precede the capture worker's final drain.
        let mut capture = capture;
        let exit = observe_process(
            &core,
            &id,
            || poll_capture(&mut child, &capture),
            STOP_GRACE,
            STOP_CONFIRM,
        );
        // Reap already revoked signal ownership. Keep the slot until terminal
        // capture ends; build history is finalized only by BES.
        let captured = capture.finish();
        let mut st = core.state.lock().expect("application state lock poisoned");
        finish_console(&mut st, &id);
        if apply_reap(&mut st, &id, &exit)
            && let Err(error) = captured
        {
            st.run_note = Some(format!("Failed to save the terminal output: {error}"));
        }
    });
    Ok(rec)
}

/// Process ownership returned separately from UI notification and observation.
struct LaunchedRun {
    record: RunRecord,
    child: Child,
    capture: crate::terminal::Capture,
}

/// Validate before touching storage, then reserve, spawn and publish atomically.
fn launch(
    core: &Core,
    data: &Path,
    cmd: &str,
    target: &str,
    arguments: &str,
) -> Result<LaunchedRun, String> {
    validate_cmd(cmd)?;
    validate_target(target)?;
    let arguments = parse_arguments(arguments)?;
    let mut st = core.state.lock().expect("application state lock poisoned");
    let (Some(ws), Some(bazel)) = (st.workspace.clone(), st.bazel.clone()) else {
        return Err("no workspace or bazel".to_string());
    };
    if st.ingest_running || st.run.is_some() {
        return Err("bazel is busy".to_string());
    }
    let endpoint = receiver_endpoint(&st)?;
    // Held across the record write and the spawn, so that the busy check above
    // and the slot below stay one atomic step, without a `starting` flag.
    // The record exists before bazel does; a failing store prevents the spawn.
    let id = uuid::Uuid::new_v4().to_string();
    let dir = store::reserve_invocation(data, &id).map_err(|e| e.to_string())?;
    let rec = RunRecord {
        id,
        workspace: None,
        cmd: None,
        target: None,
        started_at: None,
        finished_at: None,
        exit_code: None,
    };
    // Its own process group: a stop reaches bazel and whatever it started.
    let (child, terminal, capture) = match crate::terminal::spawn(
        Command::new(&bazel)
            .args(argv(cmd, target, &arguments, &dir, endpoint, &rec.id))
            .current_dir(&ws),
        &dir,
    ) {
        Ok(c) => c,
        Err(e) => {
            let _ = store::abort_reservation(data, &rec.id);
            return Err(format!("failed to spawn {}: {e}", bazel.display()));
        }
    };
    st.run = Some(ActiveRun {
        id: rec.id.clone().into(),
        terminal,
        pgid: Some(child.id() as i32),
        stopping: false,
        stop_started: None,
    });
    st.console = Some(ConsoleSession {
        id: rec.id.clone().into(),
        workspace: ws,
        finished: false,
    });
    st.run_note = None;
    drop(st);
    Ok(LaunchedRun {
        record: rec,
        child,
        capture,
    })
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

fn finish_console(st: &mut CoreState, id: &str) {
    if let Some(console) = &mut st.console
        && console.id.as_str() == id
    {
        console.finished = true;
    }
}

/// Stop the run in flight: SIGINT to its process group, SIGKILL after
/// `STOP_GRACE` (3 s) and, if the exit is still unconfirmed `STOP_CONFIRM`
/// (2 s) later, the slot released with the note. Does nothing when no run is
/// held or a stop is already in flight, so the slot is never held by a stop.
pub fn stop(app: AppHandle, core: Arc<Core>) -> Result<(), String> {
    begin_stop(&mut core.state.lock().expect("application state lock poisoned"));
    core.emit(&app);
    Ok(())
}

/// The first request sends SIGINT and fixes the deadlines. Only the thread
/// owning the child can reap or escalate, so a reap cannot race a later signal.
fn begin_stop(st: &mut CoreState) {
    let Some(run) = st.run.as_mut() else {
        return;
    };
    let Some(pgid) = run.pgid.filter(|_| !run.stopping) else {
        return;
    };
    run.stopping = true;
    run.stop_started = Some(Instant::now());
    st.run_note = None;
    if let Err(e) = signal_run(st, pgid, Signal::INT) {
        log::warn!("SIGINT to run group failed: {e}");
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
        if !still_held(&st, id) {
            return Err(io::Error::other("run no longer owned"));
        }
        match poll() {
            Ok(Some(code)) => {
                if let Some(run) = st.run.as_mut() {
                    run.pgid = None;
                }
                return Ok(code);
            }
            Ok(None) => (),
            Err(error) => {
                // A failed wait can mean another reaper already consumed the
                // child. Its former PID is no longer safe to signal.
                if let Some(run) = st.run.as_mut() {
                    run.pgid = None;
                }
                return Err(error);
            }
        }
        if let Some(started) = st.run.as_ref().and_then(|run| run.stop_started) {
            if started.elapsed() >= grace && !killed {
                kill_run(&st);
                killed = true;
            }
            if started.elapsed() >= grace + confirm {
                if let Some(run) = st.run.as_mut() {
                    run.pgid = None;
                }
                return Err(io::Error::new(io::ErrorKind::TimedOut, STOP_UNCONFIRMED));
            }
        }
        drop(st);
        thread::sleep(EXIT_POLL);
    }
}

/// A panic in observation or saving must release the slot and wake app exit.
/// Normal completion publishes after the slot is released.
struct RunSlot<'a, F: Fn()> {
    core: &'a Core,
    id: &'a str,
    emit: F,
}
impl<F: Fn()> Drop for RunSlot<'_, F> {
    fn drop(&mut self) {
        let mut st = self
            .core
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        finish_console(&mut st, self.id);
        let held = still_held(&st, self.id);
        if held {
            clear_run(&mut st);
        }
        if held && thread::panicking() {
            st.run_note = Some("Failed to finish processing the run result".into());
        }
        self.core.run_finished.notify_all();
        drop(st);
        // Avoid a second panic from an event emission while unwinding.
        if !thread::panicking() {
            (self.emit)();
        }
    }
}

/// Bound process stopping to the short exit grace. A child already reaped has
/// nothing left to signal; wait for its in-progress save to release the slot.
pub fn exit_stop(core: &Core) {
    let id = {
        let mut st = core.state.lock().expect("application state lock poisoned");
        begin_stop(&mut st);
        st.run.as_ref().map(|run| run.id.clone())
    };
    let Some(id) = id else {
        return;
    };
    let deadline = Instant::now() + EXIT_GRACE;
    loop {
        let st = core.state.lock().expect("application state lock poisoned");
        if !still_held(&st, &id) {
            return;
        }
        if st.run.as_ref().is_some_and(|run| run.pgid.is_none()) {
            wait_for_save(core, st, &id);
            return;
        }
        if Instant::now() >= deadline {
            kill_run(&st);
            return;
        }
        drop(st);
        thread::sleep(EXIT_POLL);
    }
}

fn wait_for_save(core: &Core, mut state: std::sync::MutexGuard<'_, CoreState>, id: &str) {
    while still_held(&state, id) {
        state = core
            .run_finished
            .wait(state)
            .unwrap_or_else(|error| error.into_inner());
    }
}

fn kill_run(state: &CoreState) {
    if let Some(pgid) = state.run.as_ref().and_then(|run| run.pgid)
        && let Err(error) = signal_run(state, pgid, Signal::KILL)
    {
        log::warn!("SIGKILL to run group failed: {error}");
    }
}

/// Whether `id` is still the run in flight: only that run's group may be
/// signalled and only its slot may be released.
fn still_held(st: &CoreState, id: &str) -> bool {
    st.run.as_ref().is_some_and(|run| run.id.as_str() == id)
}

/// Give up on the run `id` after an unconfirmed kill: release its slot and put
/// up the note that says the exit is unknown. Returns whether it did; a
/// successor run in the slot is never touched.
fn give_up(st: &mut CoreState, id: &str) -> bool {
    if !still_held(st, id) {
        return false;
    }
    clear_run(st);
    st.run_note = Some(STOP_UNCONFIRMED.to_string());
    true
}

/// Apply the reap of `id` to the state and return whether this reap belongs to
/// the run still holding the slot. A `wait` error leaves the process exit
/// unknown, with the reason in the note. A successor run is never touched.
fn apply_reap(st: &mut CoreState, id: &str, exit: &std::io::Result<i32>) -> bool {
    if !still_held(st, id) {
        return false;
    }
    if exit
        .as_ref()
        .is_err_and(|error| error.kind() == io::ErrorKind::TimedOut)
    {
        return give_up(st, id);
    }
    clear_run(st);
    st.run_note = match exit {
        Ok(_) => None,
        Err(e) => Some(format!("Could not confirm the exit: {e}")),
    };
    true
}

/// Clear the run slot: nothing is in flight, nothing is ours to signal. The
/// note is the caller's: a recorded exit clears it, an unknown one sets it.
fn clear_run(st: &mut CoreState) {
    st.run = None;
}

/// A run's process group must not address the caller's group (0) or all
/// signalable processes (1 when negated by kill_process_group).
fn process_group(pgid: i32) -> io::Result<Pid> {
    if pgid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid run process group",
        ));
    }
    Ok(Pid::from_raw(pgid).expect("run process group is positive"))
}

/// Signal the run's session group, or its terminal's foreground job group.
fn signal_group(pgid: i32, signal: Signal) -> io::Result<()> {
    Ok(process::kill_process_group(process_group(pgid)?, signal)?)
}

fn signal_run(st: &CoreState, pgid: i32, signal: Signal) -> io::Result<()> {
    if let Some(foreground) = st
        .run
        .as_ref()
        .and_then(|run| run.terminal.foreground_group())
        && foreground != pgid
    {
        let _ = signal_group(foreground, signal);
    }
    signal_group(pgid, signal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held_id(st: &CoreState) -> Option<&str> {
        st.run.as_ref().map(|run| run.id.as_str())
    }

    fn held_pgid(st: &CoreState) -> Option<i32> {
        st.run.as_ref().and_then(|run| run.pgid)
    }

    fn is_stopping(st: &CoreState) -> bool {
        st.run.as_ref().is_some_and(|run| run.stopping)
    }

    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;

    struct LogDir(std::path::PathBuf);

    impl LogDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("zeal-run-log-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            fs::write(path.join("terminal.log"), b"").unwrap();
            Self(path)
        }
    }

    impl Drop for LogDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn terminal_saves_both_streams_and_non_utf8_bytes() {
        let dir = LogDir::new();
        let (mut child, _terminal, mut capture) = crate::terminal::spawn(
            Command::new("sh")
                .arg("-c")
                .arg("printf 'out\\r\\n'; printf 'err\\n' >&2; printf '\\377tail'"),
            &dir.0,
        )
        .unwrap();
        let _guard = GroupGuard(child.id() as i32);
        assert!(child.stdout.is_none());
        assert!(child.stderr.is_none());
        assert!(child.wait().unwrap().success());
        capture.finish().unwrap();
        assert_eq!(
            fs::read(dir.0.join("terminal.log")).unwrap(),
            b"out\r\r\nerr\r\n\xfftail"
        );
        assert_eq!(
            String::from_utf8_lossy(&fs::read(dir.0.join("terminal.log")).unwrap()),
            "out\r\r\nerr\r\n\u{fffd}tail"
        );
    }

    #[test]
    fn parent_exit_does_not_wait_for_a_descendant_holding_the_log_open() {
        let dir = LogDir::new();
        let (mut child, _terminal, mut capture) = crate::terminal::spawn(
            Command::new("sh")
                .arg("-c")
                .arg("trap '' HUP; sleep 30 & printf 'done\\n'; exit 7"),
            &dir.0,
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
        assert_eq!(
            String::from_utf8_lossy(&fs::read(dir.0.join("terminal.log")).unwrap()),
            "done\r\n"
        );
    }

    #[test]
    fn missing_log_prevents_the_command_from_starting() {
        let dir = LogDir::new();
        fs::remove_file(dir.0.join("terminal.log")).unwrap();
        let marker = dir.0.join("spawned");
        let result = crate::terminal::spawn(Command::new("touch").arg(&marker), &dir.0);
        assert!(result.is_err());
        assert!(!marker.exists());
    }

    #[test]
    fn argv_has_the_command_one_target_and_the_observation_flags() {
        let dir = Path::new("/data/runs/abc");
        for (cmd, target) in [("build", "//pkg:b"), ("test", "//pkg:t")] {
            let args = argv(cmd, target, &[], dir, "grpc://127.0.0.1:12345", "abc");
            assert_eq!(args[0], cmd);
            assert_eq!(args.last().unwrap(), target);
            assert_eq!(
                &args[1..args.len() - 1],
                [
                    "--bes_backend=grpc://127.0.0.1:12345",
                    "--bes_proxy=",
                    "--bes_upload_mode=wait_for_upload_complete",
                    "--profile=/data/runs/abc/profile.gz",
                    "--experimental_profile_include_primary_output",
                    "--noslim_profile",
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
        let data = temp.0.join("data");
        let id = uuid::Uuid::new_v4().to_string();
        let dir = store::reserve_invocation(&data, &id).unwrap();
        let (mut child, terminal, mut capture) = crate::terminal::spawn(
            Command::new("sh").args(["-c", "printf 'startup failed\\n' >&2; exit 7"]),
            &dir,
        )
        .unwrap();
        let core = Core::new();
        {
            let mut state = core.state.lock().unwrap();
            state.workspace = Some(ws.clone());
            state.run = Some(ActiveRun {
                id: id.clone().into(),
                terminal,
                pgid: Some(child.id() as i32),
                stopping: false,
                stop_started: None,
            });
            state.console = Some(ConsoleSession {
                id: id.clone().into(),
                workspace: ws.clone(),
                finished: false,
            });
        }
        let exit = Ok(child.wait().unwrap().code().unwrap());
        capture.finish().unwrap();
        {
            let mut state = core.state.lock().unwrap();
            finish_console(&mut state, &id);
            assert!(apply_reap(&mut state, &id, &exit));
            begin_stop(&mut state);
            assert!(!is_stopping(&state));
        }
        assert!(store::read_record(&data, &ws, &id).is_err());
        assert!(crate::terminal::write(&core, &id, b"late input").is_err());
        assert!(crate::terminal::resize(&core, &id, 80, 24).is_ok());
        let output =
            serde_json::to_value(crate::console::read(&data, &core, &id, 0).unwrap()).unwrap();
        let bytes: Vec<u8> = serde_json::from_value(output["data"].clone()).unwrap();
        assert_eq!(bytes, b"startup failed\r\n");
        assert_eq!(output["done"], true);
        assert_eq!(core.dto().console_run.as_deref(), Some(id.as_str()));
        let reopened =
            serde_json::to_value(crate::console::read(&data, &core, &id, 0).unwrap()).unwrap();
        assert_eq!(reopened, output);
    }

    #[test]
    fn an_old_capture_cannot_finish_or_restore_a_new_console() {
        let core = Core::new();
        let mut state = core.state.lock().unwrap();
        state.console = Some(ConsoleSession {
            id: "new".into(),
            workspace: Path::new("/workspace").into(),
            finished: false,
        });
        finish_console(&mut state, "old");
        assert!(!state.console.as_ref().unwrap().finished);
        finish_console(&mut state, "new");
        assert!(state.console.as_ref().unwrap().finished);
        state.console = None;
        finish_console(&mut state, "new");
        assert!(state.console.is_none());
    }

    /// A real local process boundary. The fixed script only records its argv;
    /// production input is never interpolated into shell source.
    struct LaunchFixture {
        temp: LogDir,
        ws: std::path::PathBuf,
        data: std::path::PathBuf,
        core: Arc<Core>,
    }

    impl LaunchFixture {
        fn new() -> Self {
            let temp = LogDir::new();
            let ws = temp.0.join("workspace");
            fs::create_dir(&ws).unwrap();
            let core = Arc::new(Core::new());
            let mut state = core.state.lock().unwrap();
            state.workspace = Some(ws.clone());
            state.bes_endpoint = Some("grpc://127.0.0.1:12345".into());
            drop(state);
            let fixture = Self {
                data: temp.0.join("data 日本語"),
                temp,
                ws,
                core,
            };
            fixture.executable("#!/bin/sh\nprintf '%s\\000' \"$@\" > argv.bin\n");
            fixture
        }

        fn executable(&self, source: &str) {
            use std::os::unix::fs::PermissionsExt;
            let path = self.temp.0.join("bazel executable");
            fs::write(&path, source).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            self.core.state.lock().unwrap().bazel = Some(path);
        }

        fn try_launch(&self, cmd: &str, target: &str, arguments: &str) -> Result<OwnedRun, String> {
            launch(&self.core, &self.data, cmd, target, arguments).map(|process| OwnedRun {
                process,
                core: self.core.clone(),
            })
        }

        fn launch(&self, cmd: &str, target: &str, arguments: &str) -> OwnedRun {
            self.try_launch(cmd, target, arguments).unwrap()
        }

        fn recorded_arguments(&self) -> Vec<String> {
            let recorded = fs::read(self.ws.join("argv.bin")).unwrap();
            recorded
                .strip_suffix(&[0])
                .unwrap()
                .split(|byte| *byte == 0)
                .map(|word| String::from_utf8(word.to_vec()).unwrap())
                .collect()
        }
    }

    /// Own the child through polling and unwinding. No timed-out worker can
    /// retain it after the test moves on.
    struct OwnedRun {
        process: LaunchedRun,
        core: Arc<Core>,
    }

    impl OwnedRun {
        async fn finish(&mut self, deadline: Instant) -> io::Result<i32> {
            let exit = wait_for_child(self, deadline).await;
            self.process.capture.finish()?;
            let mut state = self.core.state.lock().unwrap();
            finish_console(&mut state, &self.process.record.id);
            apply_reap(&mut state, &self.process.record.id, &exit);
            exit
        }

        fn output(&self, data: &Path) -> String {
            use std::io::Read;
            let mut output = String::new();
            store::live_artifact(data, &self.process.record.id, "terminal.log")
                .unwrap()
                .read_to_string(&mut output)
                .unwrap();
            output
        }
    }

    async fn wait_for_child(run: &mut OwnedRun, deadline: Instant) -> io::Result<i32> {
        let mut status = run.process.child.try_wait()?;
        while status.is_none() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
            status = run.process.child.try_wait()?;
        }
        if let Some(status) = status {
            return Ok(status.code().unwrap_or(-1));
        }
        terminate_child(&mut run.process.child);
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "child deadline expired",
        ))
    }

    fn terminate_child(child: &mut Child) {
        if matches!(child.try_wait(), Ok(None)) {
            let _ = signal_group(child.id() as i32, Signal::KILL);
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    impl Drop for OwnedRun {
        fn drop(&mut self) {
            terminate_child(&mut self.process.child);
            let _ = self.process.capture.finish();
        }
    }

    #[tokio::test]
    async fn launch_preserves_program_arguments_without_shell_expansion() {
        let fixture = LaunchFixture::new();
        let mut run = fixture.launch(
            "run",
            "//:app",
            r#"--config dev -- "two words" '' $HOME '$(touch injected)' `id` ; | > * '#'"#,
        );
        assert_eq!(
            run.finish(Instant::now() + Duration::from_secs(5))
                .await
                .unwrap(),
            0
        );
        let arguments = fixture.recorded_arguments();
        let target = arguments.iter().position(|word| word == "//:app").unwrap();
        assert_eq!(arguments[0], "run");
        assert!(
            arguments[..target].contains(&format!("--invocation_id={}", run.process.record.id))
        );
        assert_eq!(
            &arguments[target + 1..],
            [
                "--config",
                "dev",
                "--",
                "two words",
                "",
                "$HOME",
                "$(touch injected)",
                "`id`",
                ";",
                "|",
                ">",
                "*",
                "#"
            ]
        );
        assert!(!fixture.ws.join("injected").exists());
    }

    #[tokio::test]
    async fn launch_rejects_unclosed_quotes_without_side_effects_and_allows_retry() {
        let fixture = LaunchFixture::new();
        let error = fixture
            .try_launch("run", "//:app", "'unfinished")
            .err()
            .expect("invalid input must fail");
        assert!(error.contains("Unclosed quote"));
        assert!(!fixture.data.exists());
        assert!(!fixture.ws.join("argv.bin").exists());
        assert!(fixture.core.state.lock().unwrap().run.is_none());
        assert!(fixture.core.state.lock().unwrap().console.is_none());

        let mut run = fixture.launch("run", "//:app", "-- 'corrected value'");
        assert_eq!(
            run.finish(Instant::now() + Duration::from_secs(5))
                .await
                .unwrap(),
            0
        );
        let arguments = fixture.recorded_arguments();
        assert_eq!(
            &arguments[arguments.len() - 3..],
            ["//:app", "--", "corrected value"]
        );
    }

    #[tokio::test]
    async fn launch_rejects_nul_without_side_effects_and_allows_retry() {
        let fixture = LaunchFixture::new();
        let error = fixture
            .try_launch("run", "//:app", "-- bad\0value")
            .err()
            .expect("NUL input must fail");
        assert!(error.contains("NUL"));
        assert!(!fixture.data.exists());
        assert!(!fixture.ws.join("argv.bin").exists());
        assert!(fixture.core.state.lock().unwrap().run.is_none());
        assert!(fixture.core.state.lock().unwrap().console.is_none());

        let mut run = fixture.launch("run", "//:app", "-- corrected");
        assert_eq!(
            run.finish(Instant::now() + Duration::from_secs(5))
                .await
                .unwrap(),
            0
        );
        let arguments = fixture.recorded_arguments();
        assert_eq!(
            &arguments[arguments.len() - 3..],
            ["//:app", "--", "corrected"]
        );
    }

    #[tokio::test]
    async fn child_deadline_kills_and_reaps_the_owned_process() {
        let fixture = LaunchFixture::new();
        fixture.executable("#!/bin/sh\nexec /bin/sleep 30\n");
        let mut run = fixture.launch("build", "//:app", "");
        assert_eq!(
            run.finish(Instant::now()).await.unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(run.process.child.try_wait().unwrap().is_some());
        assert!(fixture.core.state.lock().unwrap().run.is_none());
    }

    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary; may download dependencies"]
    async fn real_bazel_gui_build_uses_config() {
        let fixture = GuiBazelFixture::new().await;
        fixture.config("arguments", "--define=message=hello");
        let mut run = fixture
            .base
            .launch("build", "//:double", "--config=arguments");
        assert_eq!(
            run.finish(Instant::now() + Duration::from_secs(60))
                .await
                .unwrap(),
            0,
            "{}",
            run.output(&fixture.base.data)
        );
        let record =
            store::read_record(&fixture.base.data, &fixture.base.ws, &run.process.record.id)
                .unwrap();
        assert_eq!(record.target.as_deref(), Some("//:double"));
        assert_eq!(record.exit_code, Some(0));
        let configured = fs::read_to_string(fixture.base.ws.join("bazel-bin/double.py")).unwrap();
        assert!(configured.contains("CONFIG=hello"));
        assert!(
            store::artifact(
                &fixture.base.data,
                &fixture.base.ws,
                &record.id,
                "profile.gz"
            )
            .is_ok()
        );
    }

    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary; may download dependencies"]
    async fn real_bazel_gui_test_uses_filter() {
        let fixture = GuiBazelFixture::new().await;
        let mut run = fixture
            .base
            .launch("test", "//:check", "--test_filter='suite one'");
        assert_eq!(
            run.finish(Instant::now() + Duration::from_secs(60))
                .await
                .unwrap(),
            0,
            "{}",
            run.output(&fixture.base.data)
        );
        let record =
            store::read_record(&fixture.base.data, &fixture.base.ws, &run.process.record.id)
                .unwrap();
        assert_eq!(record.cmd.as_deref(), Some("test"));
        assert_eq!(record.exit_code, Some(0));
        let log =
            fs::read_to_string(fixture.base.ws.join("bazel-testlogs/check/test.log")).unwrap();
        assert!(log.contains("FILTER=<suite one>"), "{log}");
    }

    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary; may download dependencies"]
    async fn real_bazel_gui_run_preserves_program_arguments() {
        let fixture = GuiBazelFixture::new().await;
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut run = fixture.base.launch(
            "run",
            "//:double",
            r#"-- "two words" '' '$HOME;$(touch injected)' `id` | > * '#'"#,
        );
        let record = wait_for_gui_build(&fixture, &mut run, deadline).await;
        assert_eq!(
            record.exit_code,
            Some(0),
            "build finishes before program input"
        );
        assert!(run.process.child.try_wait().unwrap().is_none());
        crate::terminal::write(&fixture.base.core, &record.id, b"21\n").unwrap();
        assert_eq!(run.finish(deadline).await.unwrap(), 23);
        let output = run.output(&fixture.base.data);
        let reported = output
            .lines()
            .find_map(|line| line.strip_prefix("ARGV="))
            .expect("program reports its argv");
        let received: Vec<String> = serde_json::from_str(reported).unwrap();
        assert_eq!(
            received,
            [
                "two words",
                "",
                "$HOME;$(touch injected)",
                "`id`",
                "|",
                ">",
                "*",
                "#"
            ]
        );
        assert!(output.contains("42"), "{output}");
        assert!(!fixture.base.ws.join("injected").exists());
        assert_eq!(
            store::read_record(&fixture.base.data, &fixture.base.ws, &record.id)
                .unwrap()
                .exit_code,
            Some(0)
        );
        assert!(fixture.base.core.state.lock().unwrap().run.is_none());
        assert_saved_console(&fixture.base.data, &fixture.base.core, &record.id, &output);
    }

    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary; may download dependencies"]
    async fn real_bazel_gui_rejects_missing_config_value() {
        let fixture = GuiBazelFixture::new().await;
        let mut run = fixture.base.launch("build", "//:double", "--config");
        assert_eq!(
            run.finish(Instant::now() + Duration::from_secs(30))
                .await
                .unwrap(),
            2
        );
        assert!(
            run.output(&fixture.base.data)
                .contains("Expected value after --config")
        );
        assert!(fixture.base.core.state.lock().unwrap().run.is_none());
        assert!(
            store::read_record(&fixture.base.data, &fixture.base.ws, &run.process.record.id)
                .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary; may download dependencies"]
    async fn real_bazel_gui_rejects_missing_filter_value() {
        let fixture = GuiBazelFixture::new().await;
        let mut run = fixture.base.launch("test", "//:check", "--test_filter");
        assert_eq!(
            run.finish(Instant::now() + Duration::from_secs(30))
                .await
                .unwrap(),
            2
        );
        assert!(
            run.output(&fixture.base.data)
                .contains("Expected value after --test_filter")
        );
        assert!(fixture.base.core.state.lock().unwrap().run.is_none());
        assert!(
            store::read_record(&fixture.base.data, &fixture.base.ws, &run.process.record.id)
                .is_err()
        );
    }

    struct GuiBazelFixture {
        base: LaunchFixture,
        server: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    }

    impl Drop for GuiBazelFixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    impl GuiBazelFixture {
        async fn new() -> Self {
            let base = LaunchFixture::new();
            let bazel = std::env::var("ZEAL_TEST_BAZEL").expect("ZEAL_TEST_BAZEL");
            let startup = shlex::try_join([
                bazel.as_str(),
                "--batch",
                "--nosystem_rc",
                "--nohome_rc",
                &format!("--output_user_root={}", base.temp.0.join("bazel").display()),
            ])
            .unwrap();
            base.executable(&format!("#!/bin/sh\nexec {startup} \"$@\"\n"));
            fs::write(
                base.ws.join("WORKSPACE"),
                "workspace(name='zeal_gui_test')\n",
            )
            .unwrap();
            fs::write(base.ws.join(".bazelrc"), "build --enable_bzlmod=false\nbuild --enable_workspace=true\nbuild --incompatible_autoload_externally=\nbuild --host_platform=//:host\nbuild --platforms=//:host\nbuild --spawn_strategy=local\nbuild --bes_timeout=10s\nbuild --bes_proxy=unix:/zeal-test-no-proxy.sock\nbuild --bes_backend=grpc://127.0.0.1:1\n").unwrap();
            fs::write(base.ws.join("BUILD"), "load(':binary.bzl','binary','check_test')\nplatform(name='host')\nbinary(name='double')\ncheck_test(name='check')\n").unwrap();
            fs::write(base.ws.join("binary.bzl"), r#"def _binary(ctx):
    out=ctx.actions.declare_file(ctx.label.name+'.py')
    source='#!/usr/bin/env python3\nimport json,sys\nprint("CONFIG=%s")\nprint("ARGV="+json.dumps(sys.argv[1:],separators=(",",":")))\nprint(int(input())*2)\nraise SystemExit(23)\n' % ctx.var.get('message','unset')
    ctx.actions.write(out,source,is_executable=True)
    return [DefaultInfo(executable=out)]
def _check(ctx):
    out=ctx.actions.declare_file(ctx.label.name+'.sh')
    ctx.actions.write(out,'#!/bin/sh\nprintf "FILTER=<%s>\\n" "$TESTBRIDGE_TEST_ONLY"\n',is_executable=True)
    return [DefaultInfo(executable=out)]
binary=rule(implementation=_binary,executable=True)
check_test=rule(implementation=_check,test=True)
"#).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("grpc://{}", listener.local_addr().unwrap());
            base.core.state.lock().unwrap().bes_endpoint = Some(endpoint);
            let server = tokio::spawn(crate::bes::serve(listener, base.data.clone()));
            Self { base, server }
        }

        fn config(&self, name: &str, flags: &str) {
            use std::io::Write;
            let mut rc = fs::OpenOptions::new()
                .append(true)
                .open(self.base.ws.join(".bazelrc"))
                .unwrap();
            writeln!(rc, "build:{name} {flags}").unwrap();
        }
    }

    async fn wait_for_gui_build(
        fixture: &GuiBazelFixture,
        run: &mut OwnedRun,
        deadline: Instant,
    ) -> RunRecord {
        let id = &run.process.record.id;
        let mut record = store::read_record(&fixture.base.data, &fixture.base.ws, id);
        while !record
            .as_ref()
            .is_ok_and(|record| record.finished_at.is_some())
        {
            assert!(
                run.process.child.try_wait().unwrap().is_none(),
                "Bazel ended before BEP receipt: {}",
                run.output(&fixture.base.data)
            );
            assert!(
                Instant::now() < deadline,
                "BEP receipt deadline: {}",
                run.output(&fixture.base.data)
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            record = store::read_record(&fixture.base.data, &fixture.base.ws, id);
        }
        record.unwrap()
    }

    /// Completed consoles expose durable bytes, but reject new input.
    fn assert_saved_console(data: &Path, core: &Core, id: &str, output: &str) {
        let console =
            serde_json::to_value(crate::console::read(data, core, id, 0).unwrap()).unwrap();
        assert_eq!(console["done"], true);
        let bytes: Vec<u8> = serde_json::from_value(console["data"].clone()).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), output);
        assert!(crate::terminal::write(core, id, b"late input").is_err());
    }

    #[test]
    fn target_validation_accepts_labels_and_patterns() {
        for target in [
            "//app/lib:parser",
            "//...",
            ":all",
            ":*",
            "@maven//:guava",
            "//app/...",
            // Shell metacharacters are ordinary label characters here: nothing
            // splits or evaluates the target, so it reaches bazel verbatim as
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
            assert!(validate_target(target).is_ok(), "{target:?}");
            assert_eq!(
                argv(
                    "build",
                    target,
                    &[],
                    Path::new("/data/runs/abc"),
                    "grpc://127.0.0.1:12345",
                    "abc",
                )
                .last()
                .unwrap(),
                target
            );
        }
    }

    #[test]
    fn target_validation_rejects_empty_whitespace_control_and_leading_dash() {
        for target in [
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
            assert!(validate_target(target).is_err(), "{target:?}");
        }
        // One message for both rules, and it does not echo the input.
        let msg = validate_target("a b").unwrap_err();
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

    /// The escalation's release: the run leaves the slot with the note that
    /// says the exit is unknown, and the stopping flag goes with it.
    #[test]
    fn give_up_releases_the_run_and_says_the_exit_is_unknown() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some(ActiveRun {
            stopping: true,
            ..ActiveRun::stub("abc", Some(42))
        });
        st.run_note = Some("old".to_string());

        assert!(still_held(&st, "abc"));
        assert!(give_up(&mut st, "abc"));
        assert!(st.run.is_none());
        assert!(st.run.is_none(), "the group is no longer ours to signal");
        assert!(!is_stopping(&st));
        assert_eq!(st.run_note.as_deref(), Some(STOP_UNCONFIRMED));
    }

    /// A successor run is never touched: an escalation that outlived its run
    /// must not release the run that took the slot.
    #[test]
    fn give_up_never_touches_a_successor_run() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some(ActiveRun::stub("def", Some(7)));
        st.run_note = Some("old".to_string());

        assert!(!still_held(&st, "abc"));
        assert!(!give_up(&mut st, "abc"));
        assert_eq!(held_id(&st), Some("def"));
        assert_eq!(held_pgid(&st), Some(7));
        assert_eq!(st.run_note.as_deref(), Some("old"));
    }

    /// A reap of the run the slot still holds: the
    /// slot is cleared and the note follows the outcome.
    #[test]
    fn apply_reap_clears_the_run_it_still_holds() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some(ActiveRun {
            stopping: true,
            ..ActiveRun::stub("abc", Some(42))
        });
        st.run_note = Some("old".to_string());

        assert!(apply_reap(&mut st, "abc", &Ok(3)));
        assert!(st.run.is_none());
        assert!(st.run.is_none());
        assert!(!is_stopping(&st));
        assert_eq!(st.run_note, None, "a recorded exit clears the note");

        st.run = Some(ActiveRun::stub("abc", Some(42)));
        let why = std::io::Error::from_raw_os_error(libc::EINTR);
        assert!(apply_reap(&mut st, "abc", &Err(why)));
        assert!(st.run.is_none(), "an unknown exit still releases the slot");
        let note = st.run_note.clone().expect("the reason is shown");
        assert!(note.starts_with("Could not confirm the exit:"), "{note}");
    }

    /// A reap after the stop gave up: the slot is no longer this run's, so its
    /// give-up note stands. A successor run is
    /// not ours to reap either.
    #[test]
    fn apply_reap_after_give_up_writes_nothing() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some(ActiveRun {
            stopping: true,
            ..ActiveRun::stub("abc", Some(42))
        });
        assert!(give_up(&mut st, "abc"));

        assert!(!apply_reap(&mut st, "abc", &Ok(0)));
        assert!(st.run.is_none());
        assert_eq!(st.run_note.as_deref(), Some(STOP_UNCONFIRMED), "untouched");

        st.run = Some(ActiveRun::stub("def", Some(7)));
        assert!(!apply_reap(&mut st, "abc", &Ok(0)));
        assert_eq!(held_id(&st), Some("def"));
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
    fn run_process_groups_exclude_broadcast_and_invalid_ids() {
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
        let deadline = Instant::now() + Duration::from_secs(10);
        while group_alive(pgid) {
            assert!(Instant::now() < deadline, "group {pgid} still has members");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// `exit_stop` is the app-exit path, which cannot defer the exit: it must
    /// still kill a group that outlived its SIGINT, within the short budget.
    /// The shell ignores SIGINT (`trap '' INT`), so only the SIGKILL can empty
    /// the group.
    #[test]
    fn exit_stop_kills_the_group_after_the_short_grace() {
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
        let _guard = GroupGuard(pgid);

        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");

        // Exactly what `start` leaves for a live run.
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some(ActiveRun::stub("abc", Some(pgid)));
        }

        let started = Instant::now();
        exit_stop(&core);
        // The SIGINT did not end it, so the SIGKILL came after the grace: the
        // wait is bounded, not skipped.
        assert!(started.elapsed() >= EXIT_GRACE, "the grace was skipped");
        // A zombie still holds the group id, so reap the shell before asking
        // whether the group is empty.
        let status = child.wait().unwrap();
        assert!(!status.success(), "unexpected status: {status}");

        let deadline = Instant::now() + Duration::from_secs(10);
        while group_alive(pgid) {
            assert!(Instant::now() < deadline, "group {pgid} still has members");
            thread::sleep(Duration::from_millis(20));
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
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some(ActiveRun::stub("run", Some(pgid)));
            begin_stop(&mut st);
        }
        let exit = observe_process(
            &core,
            "run",
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
        assert!(held_pgid(&core.state.lock().unwrap()).is_none());
    }
    #[test]
    fn wait_errors_immediately_revoke_signal_ownership() {
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some(ActiveRun::stub("run", Some(i32::MAX)));
        }
        let started = Instant::now();
        let error = observe_process(
            &core,
            "run",
            || Err(io::Error::from_raw_os_error(libc::ECHILD)),
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
        assert!(started.elapsed() < Duration::from_secs(1));
        let mut st = core.state.lock().unwrap();
        assert!(held_pgid(&st).is_none());
        assert!(!is_stopping(&st));
        assert!(apply_reap(&mut st, "run", &Err(error)));
        assert!(st.run.is_none());
        assert!(
            st.run_note
                .as_ref()
                .unwrap()
                .contains("Could not confirm the exit")
        );
    }
    #[test]
    fn reap_revokes_signal_ownership_before_the_run_slot_is_released() {
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some(ActiveRun::stub("run", Some(i32::MAX)));
        }
        assert_eq!(
            observe_process(&core, "run", || Ok(Some(7)), STOP_GRACE, STOP_CONFIRM).unwrap(),
            7
        );
        let mut st = core.state.lock().unwrap();
        begin_stop(&mut st);
        assert_eq!(held_id(&st), Some("run"));
        assert!(held_pgid(&st).is_none());
        assert!(!is_stopping(&st));
    }
    #[test]
    fn repeated_stop_does_not_extend_deadlines() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some(ActiveRun::stub("run", Some(i32::MAX)));
        begin_stop(&mut st);
        let first = st.run.as_ref().and_then(|run| run.stop_started);
        begin_stop(&mut st);
        assert_eq!(st.run.as_ref().and_then(|run| run.stop_started), first);
    }
    #[test]
    fn old_owner_never_polls_or_signals_a_successor() {
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some(ActiveRun::stub("next", Some(i32::MAX)));
        }
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
        assert_eq!(held_id(&st), Some("next"));
        assert_eq!(held_pgid(&st), Some(i32::MAX));
    }
    #[test]
    fn panic_releases_the_run_slot_and_wakes_exit() {
        let core = Arc::new(Core::new());
        core.state.lock().unwrap().run = Some(ActiveRun::stub("run", None));
        let exiting_core = core.clone();
        let exiting = thread::spawn(move || exit_stop(&exiting_core));
        let owning_core = core.clone();
        let owner = thread::spawn(move || {
            let _slot = RunSlot {
                core: &owning_core,
                id: "run",
                emit: || (),
            };
            panic!("injected save panic");
        });
        assert!(owner.join().is_err());
        exiting.join().unwrap();
        assert!(core.state.lock().unwrap().run.is_none());
        assert!(
            core.state
                .lock()
                .unwrap()
                .run_note
                .as_ref()
                .unwrap()
                .contains("finish processing")
        );
    }
    #[test]
    fn a_stop_racing_with_reap_cannot_signal_the_reaped_pid() {
        let core = Arc::new(Core::new());
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some(ActiveRun::stub("run", Some(i32::MAX)));
        }
        let (polling_tx, polling_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let owner_core = core.clone();
        let poll = move || {
            polling_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(Some(0))
        };
        let owner = thread::spawn(move || {
            observe_process(&owner_core, "run", poll, STOP_GRACE, STOP_CONFIRM)
        });
        polling_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (stopping_tx, stopping_rx) = mpsc::sync_channel(1);
        let stopping_core = core.clone();
        let stopping = thread::spawn(move || {
            stopping_tx.send(()).unwrap();
            begin_stop(&mut stopping_core.state.lock().unwrap());
        });
        stopping_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(owner.join().unwrap().unwrap(), 0);
        stopping.join().unwrap();
        let st = core.state.lock().unwrap();
        assert!(held_pgid(&st).is_none());
        assert!(!is_stopping(&st));
    }
    #[test]
    fn app_exit_waits_for_a_reaped_runs_save_and_never_signals_it() {
        let core = Arc::new(Core::new());
        core.state.lock().unwrap().run = Some(ActiveRun::stub("run", None));
        let (finish_tx, finish_rx) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let finishing_core = core.clone();
        let finishing = thread::spawn(move || {
            let _slot = RunSlot {
                core: &finishing_core,
                id: "run",
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
        assert!(st.run.is_none());
        assert!(!is_stopping(&st));
        assert!(held_pgid(&st).is_none());
    }
    #[test]
    fn unconfirmed_kill_expires_without_releasing_a_successors_slot() {
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some(ActiveRun::stub("run", Some(i32::MAX)));
            begin_stop(&mut st);
        }
        let exit = observe_process(&core, "run", || Ok(None), Duration::ZERO, Duration::ZERO);
        assert_eq!(exit.as_ref().unwrap_err().kind(), io::ErrorKind::TimedOut);
        let mut st = core.state.lock().unwrap();
        assert!(held_pgid(&st).is_none());
        assert!(apply_reap(&mut st, "run", &exit));
        assert!(st.run.is_none());
        assert_eq!(st.run_note.as_deref(), Some(STOP_UNCONFIRMED));
        st.run = Some(ActiveRun::stub("next", Some(i32::MAX)));
        assert!(!apply_reap(&mut st, "run", &exit));
        assert_eq!(held_id(&st), Some("next"));
        assert_eq!(held_pgid(&st), Some(i32::MAX));
    }
}
