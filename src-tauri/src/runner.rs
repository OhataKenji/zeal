//! Runs `bazel build|test|run <target>` and captures terminal.log.
//! All commands use a PTY; output is saved independently of the Console view.
//! Build history is received by the app-owned BES, independently of process exit.
//!
//! Stop is best-effort: SIGINT, then SIGKILL after a grace period. An unknown
//! exit releases the slot with a note. Only
//! the currently held run may be signalled; descendants are not supervised.

use crate::state::{ConsoleSession, Core, CoreState};
use crate::store::{self, RunRecord};
use rustix::process::{self, Pid, Signal};
#[cfg(test)]
use std::fs;
use std::io;
#[cfg(test)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
#[cfg(test)]
use std::process::Stdio;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Commands the composer offers. Zeal assembles every other argument, so a
/// free-form command line never reaches bazel.
const COMMANDS: [&str; 3] = ["build", "test", "run"];

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

/// Arguments after the bazel executable. Bazel writes its artifacts into the
/// run dir.
fn argv(cmd: &str, target: &str, dir: &Path, endpoint: &str) -> Vec<String> {
    vec![
        cmd.to_string(),
        target.to_string(),
        format!("--bes_backend={endpoint}"),
        "--bes_proxy=".to_string(),
        "--bes_upload_mode=wait_for_upload_complete".to_string(),
        format!("--profile={}", dir.join("profile.gz").display()),
        "--experimental_profile_include_primary_output".to_string(),
        "--noslim_profile".to_string(),
        "--tool_tag=zeal".to_string(),
        "--color=yes".to_string(),
        "--curses=no".to_string(),
    ]
}

/// Only the composer's own commands: the observation flags belong to Zeal, so
/// anything else would be a command line this core does not model.
fn validate_cmd(cmd: &str) -> Result<(), String> {
    if COMMANDS.contains(&cmd) {
        Ok(())
    } else {
        Err(format!("unsupported command: {cmd}"))
    }
}

/// The target is passed to bazel as one `Command::arg`: nothing is split,
/// quoted or escaped, so whitespace, control characters and a leading `-` are
/// rejected instead of interpreted. Patterns such as `//...` or `:all` are
/// bazel's business. The message names both rules, without echoing the input,
/// which may hold control characters; it must stay in step with the UI's
/// `targetError`. That rule — Unicode White_Space plus Cc — is what
/// `char::is_whitespace` and `char::is_control` test here, and the TS regexp
/// `/[\p{White_Space}\p{Cc}]/u` is its exact counterpart.
fn validate_target(target: &str) -> Result<(), String> {
    let ok = !target.is_empty()
        && !target.starts_with('-')
        && !target.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(())
    } else {
        Err(
            "target must be one label or pattern, without whitespace or control \
             characters, and must not start with '-'"
                .to_string(),
        )
    }
}

/// Start a run in the open workspace and return its record. The run finishes
/// in the background; target data is refreshed only on request.
pub fn start(
    app: AppHandle,
    core: Arc<Core>,
    data: &Path,
    cmd: &str,
    target: &str,
) -> Result<RunRecord, String> {
    validate_cmd(cmd)?;
    validate_target(target)?;
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
    let (mut child, terminal, capture) = match crate::terminal::spawn(
        Command::new(&bazel)
            .args(argv(cmd, target, &dir, endpoint))
            .arg(format!("--invocation_id={}", rec.id))
            .current_dir(&ws),
        &dir,
    ) {
        Ok(c) => c,
        Err(e) => {
            let _ = store::abort_reservation(data, &rec.id);
            return Err(format!("failed to spawn {}: {e}", bazel.display()));
        }
    };
    st.run = Some(rec.id.clone().into());
    st.console = Some(ConsoleSession {
        id: rec.id.clone().into(),
        workspace: ws,
        finished: false,
    });
    st.run_terminal = Some(terminal);
    st.run_pgid = Some(child.id() as i32);
    st.run_stopping = false;
    st.run_stop_started = None;
    st.run_note = None;
    drop(st);
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
    if st.run_stopping || st.run_pgid.is_none() {
        return;
    }
    st.run_stopping = true;
    st.run_stop_started = Some(Instant::now());
    st.run_note = None;
    if let Some(pgid) = st.run_pgid
        && let Err(e) = signal_run(st, pgid, Signal::INT)
    {
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
                st.run_pgid = None;
                return Ok(code);
            }
            Ok(None) => (),
            Err(error) => {
                // A failed wait can mean another reaper already consumed the
                // child. Its former PID is no longer safe to signal.
                st.run_pgid = None;
                return Err(error);
            }
        }
        if let Some(started) = st.run_stop_started {
            if started.elapsed() >= grace && !killed {
                kill_run(&st);
                killed = true;
            }
            if started.elapsed() >= grace + confirm {
                st.run_pgid = None;
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
        st.run.clone()
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
        if st.run_pgid.is_none() {
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
    if let Some(pgid) = state.run_pgid
        && let Err(error) = signal_run(state, pgid, Signal::KILL)
    {
        log::warn!("SIGKILL to run group failed: {error}");
    }
}

/// Whether `id` is still the run in flight: only that run's group may be
/// signalled and only its slot may be released.
fn still_held(st: &CoreState, id: &str) -> bool {
    st.run.as_deref() == Some(id)
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
    st.run_terminal = None;
    st.run = None;
    st.run_pgid = None;
    st.run_stopping = false;
    st.run_stop_started = None;
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
        .run_terminal
        .as_ref()
        .and_then(|terminal| terminal.foreground_group())
        && foreground != pgid
    {
        let _ = signal_group(foreground, signal);
    }
    signal_group(pgid, signal)
}

#[cfg(test)]
mod tests;
