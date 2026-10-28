//! Runs `bazel build|test|run <target>` and writes stdout/stderr to log.txt.
//! All commands use a PTY; output is saved independently of the Console view.
//! Build observations remain in the existing bep.binpb and profile files.
//!
//! Stop is best-effort: SIGINT, then SIGKILL after a grace period. An unknown
//! exit releases the slot with a note and leaves the record unfinished. Only
//! the currently held run may be signalled; descendants are not supervised.

use crate::state::{Core, CoreState};
use crate::store::{self, RunRecord};
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
fn argv(cmd: &str, target: &str, dir: &Path) -> Vec<String> {
    vec![
        cmd.to_string(),
        target.to_string(),
        format!(
            "--build_event_binary_file={}",
            dir.join("bep.binpb").display()
        ),
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
    // Held across the record write and the spawn, so that the busy check above
    // and the slot below stay one atomic step, without a `starting` flag.
    // The record exists before bazel does; a failing store prevents the spawn.
    let (rec, dir) = store::create(data, &ws, cmd, target).map_err(|e| e.to_string())?;
    // Its own process group: a stop reaches bazel and whatever it started.
    let (mut child, terminal, mut capture) = match crate::terminal::spawn(
        Command::new(&bazel)
            .args(argv(cmd, target, &dir))
            .arg(format!("--invocation_id={}", rec.id))
            .current_dir(&ws),
        &dir,
    ) {
        Ok(c) => c,
        Err(e) => {
            let _ = store::abort_create(data, &ws, &rec.id);
            return Err(format!("failed to spawn {}: {e}", bazel.display()));
        }
    };
    st.run = Some(rec.id.clone().into());
    st.run_terminal = Some(terminal);
    st.run_pgid = Some(child.id() as i32);
    st.run_stopping = false;
    st.run_stop_started = None;
    st.run_note = None;
    drop(st);
    core.emit(&app);

    let id = rec.id.clone();
    let data = data.to_path_buf();
    thread::spawn(move || {
        let _slot = RunSlot {
            core: &core,
            id: &id,
            emit: || core.emit(&app),
        };
        let exit = observe_process(
            &core,
            &id,
            || {
                let status = child.try_wait()?;
                if status.is_none() && capture.failed() {
                    // The child is still owned and unreaped. Stop it if output
                    // can no longer be captured instead of leaving it blocked.
                    let _ = signal_group(child.id() as i32, libc::SIGKILL);
                }
                Ok(status.map(|status| status.code().unwrap_or(-1)))
            },
            STOP_GRACE,
            STOP_CONFIRM,
        );
        // Reap already revoked signal ownership. Keep the run slot through the
        // save, without holding the lock across filesystem work.
        let captured = capture.finish();
        let saved = match &exit {
            Ok(code) => store::finish(&data, &ws, &id, *code).map(|_| ()),
            Err(_) => Ok(()),
        };
        let mut st = core.state.lock().expect("application state lock poisoned");
        if apply_reap(&mut st, &id, &exit)
            && let Err(e) = saved
        {
            st.run_note = Some(format!("Could not save the exit result: {e}"));
        }
        if let Err(error) = captured {
            st.run_note = Some(format!("Failed to save the terminal output: {error}"));
        }
    });
    Ok(rec)
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
        && let Err(e) = signal_run(st, pgid, libc::SIGINT)
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
                if let Some(pgid) = st.run_pgid
                    && let Err(e) = signal_run(&st, pgid, libc::SIGKILL)
                {
                    log::warn!("SIGKILL to run group failed: {e}");
                }
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
        if still_held(&st, self.id) {
            clear_run(&mut st);
            if thread::panicking() {
                st.run_note = Some("Failed to finish processing the run result".into());
            }
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
        let mut st = core.state.lock().expect("application state lock poisoned");
        if !still_held(&st, &id) {
            return;
        }
        if st.run_pgid.is_none() {
            while still_held(&st, &id) {
                st = core
                    .run_finished
                    .wait(st)
                    .unwrap_or_else(|error| error.into_inner());
            }
            return;
        }
        if Instant::now() >= deadline {
            if let Some(pgid) = st.run_pgid
                && let Err(e) = signal_run(&st, pgid, libc::SIGKILL)
            {
                log::warn!("SIGKILL to run group failed: {e}");
            }
            return;
        }
        drop(st);
        thread::sleep(EXIT_POLL);
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
/// the run still holding the slot, whose record the caller then writes. A
/// `wait` error leaves the exit unknown, with the reason in the note and no
/// code to record. When the slot no longer holds `id` — a stop that gave up
/// released it — nothing is touched and the record stays unfinished.
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

/// Signal a whole process group: `pgid` is the run's pid (`process_group(0)`
/// above), and a negative pid addresses its group.
fn signal_group(pgid: i32, signal: i32) -> std::io::Result<()> {
    // SAFETY: kill(2) only delivers a signal; it touches no memory.
    if unsafe { libc::kill(-pgid, signal) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn signal_run(st: &CoreState, pgid: i32, signal: i32) -> io::Result<()> {
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
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;

    struct LogDir(std::path::PathBuf);

    impl LogDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("zeal-run-log-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            fs::write(path.join("log.txt"), b"").unwrap();
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
            fs::read(dir.0.join("log.txt")).unwrap(),
            b"out\r\r\nerr\r\n\xfftail"
        );
        assert_eq!(
            store::read_log(&dir.0).unwrap(),
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
        assert_eq!(store::read_log(&dir.0).unwrap(), "done\r\n");
    }

    #[test]
    fn missing_log_prevents_the_command_from_starting() {
        let dir = LogDir::new();
        fs::remove_file(dir.0.join("log.txt")).unwrap();
        let marker = dir.0.join("spawned");
        let result = crate::terminal::spawn(Command::new("touch").arg(&marker), &dir.0);
        assert!(result.is_err());
        assert!(!marker.exists());
    }

    #[test]
    fn argv_has_the_command_one_target_and_the_observation_flags() {
        let dir = Path::new("/data/runs/abc");
        for (cmd, target) in [("build", "//pkg:b"), ("test", "//pkg:t")] {
            let args = argv(cmd, target, dir);
            assert_eq!(args[0], cmd);
            assert_eq!(args[1], target);
            assert_eq!(
                &args[2..],
                [
                    "--build_event_binary_file=/data/runs/abc/bep.binpb",
                    "--profile=/data/runs/abc/profile.gz",
                    "--experimental_profile_include_primary_output",
                    "--noslim_profile",
                    "--tool_tag=zeal",
                    "--color=yes",
                    "--curses=no",
                ],
                "{cmd}"
            );
        }
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
                argv("build", target, Path::new("/data/runs/abc"))[1],
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
        st.run = Some("abc".into());
        st.run_pgid = Some(42);
        st.run_stopping = true;
        st.run_note = Some("old".to_string());

        assert!(still_held(&st, "abc"));
        assert!(give_up(&mut st, "abc"));
        assert_eq!(st.run, None);
        assert_eq!(st.run_pgid, None, "the group is no longer ours to signal");
        assert!(!st.run_stopping);
        assert_eq!(st.run_note.as_deref(), Some(STOP_UNCONFIRMED));
    }

    /// A successor run is never touched: an escalation that outlived its run
    /// must not release the run that took the slot.
    #[test]
    fn give_up_never_touches_a_successor_run() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some("def".into());
        st.run_pgid = Some(7);
        st.run_note = Some("old".to_string());

        assert!(!still_held(&st, "abc"));
        assert!(!give_up(&mut st, "abc"));
        assert_eq!(st.run.as_deref(), Some("def"));
        assert_eq!(st.run_pgid, Some(7));
        assert_eq!(st.run_note.as_deref(), Some("old"));
    }

    /// A reap of the run the slot still holds: its record may be written, the
    /// slot is cleared and the note follows the outcome.
    #[test]
    fn apply_reap_clears_the_run_it_still_holds() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some("abc".into());
        st.run_pgid = Some(42);
        st.run_stopping = true;
        st.run_note = Some("old".to_string());

        assert!(apply_reap(&mut st, "abc", &Ok(3)));
        assert_eq!(st.run, None);
        assert_eq!(st.run_pgid, None);
        assert!(!st.run_stopping);
        assert_eq!(st.run_note, None, "a recorded exit clears the note");

        st.run = Some("abc".into());
        st.run_pgid = Some(42);
        let why = std::io::Error::from_raw_os_error(libc::EINTR);
        assert!(apply_reap(&mut st, "abc", &Err(why)));
        assert_eq!(st.run, None, "an unknown exit still releases the slot");
        let note = st.run_note.clone().expect("the reason is shown");
        assert!(note.starts_with("Could not confirm the exit:"), "{note}");
    }

    /// A reap after the stop gave up: the slot is no longer this run's, so its
    /// record stays unfinished and the give-up note stands. A successor run is
    /// not ours to reap either.
    #[test]
    fn apply_reap_after_give_up_writes_nothing() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some("abc".into());
        st.run_pgid = Some(42);
        st.run_stopping = true;
        assert!(give_up(&mut st, "abc"));

        assert!(!apply_reap(&mut st, "abc", &Ok(0)));
        assert_eq!(st.run, None);
        assert_eq!(st.run_note.as_deref(), Some(STOP_UNCONFIRMED), "untouched");

        st.run = Some("def".into());
        st.run_pgid = Some(7);
        assert!(!apply_reap(&mut st, "abc", &Ok(0)));
        assert_eq!(st.run.as_deref(), Some("def"));
    }

    /// Best-effort SIGKILL of the test's group, so a failed assertion cannot
    /// leave the `sleep` behind.
    struct GroupGuard(i32);

    impl Drop for GroupGuard {
        fn drop(&mut self) {
            let _ = signal_group(self.0, libc::SIGKILL);
        }
    }

    /// Signal 0 probes the group through the one helper that may: only `ESRCH`
    /// means it is empty.
    fn group_alive(pgid: i32) -> bool {
        match signal_group(pgid, 0) {
            Ok(()) => true,
            Err(e) => e.raw_os_error() != Some(libc::ESRCH),
        }
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
        signal_group(pgid, libc::SIGINT).unwrap();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("child did not exit after SIGINT");
        let status = waiter.join().unwrap().unwrap();
        assert!(!status.success(), "unexpected status: {status}");

        // A background job of a non-interactive shell may ignore SIGINT itself
        // (POSIX async list), so the group is not necessarily empty yet: kill
        // it, exactly as `stop` does after the grace, and then require every
        // member to be gone. The group may be empty already.
        let _ = signal_group(pgid, libc::SIGKILL);
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
            st.run = Some("abc".into());
            st.run_pgid = Some(pgid);
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
            st.run = Some("run".into());
            st.run_pgid = Some(pgid);
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
        assert!(core.state.lock().unwrap().run_pgid.is_none());
    }
    #[test]
    fn wait_errors_immediately_revoke_signal_ownership() {
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some("run".into());
            st.run_pgid = Some(i32::MAX);
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
        assert!(st.run_pgid.is_none());
        assert!(!st.run_stopping);
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
            st.run = Some("run".into());
            st.run_pgid = Some(i32::MAX);
        }
        assert_eq!(
            observe_process(&core, "run", || Ok(Some(7)), STOP_GRACE, STOP_CONFIRM).unwrap(),
            7
        );
        let mut st = core.state.lock().unwrap();
        begin_stop(&mut st);
        assert_eq!(st.run.as_deref(), Some("run"));
        assert!(st.run_pgid.is_none());
        assert!(!st.run_stopping);
    }
    #[test]
    fn repeated_stop_does_not_extend_deadlines() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some("run".into());
        st.run_pgid = Some(i32::MAX);
        begin_stop(&mut st);
        let first = st.run_stop_started;
        begin_stop(&mut st);
        assert_eq!(st.run_stop_started, first);
    }
    #[test]
    fn old_owner_never_polls_or_signals_a_successor() {
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some("next".into());
            st.run_pgid = Some(i32::MAX);
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
        assert_eq!(st.run.as_deref(), Some("next"));
        assert_eq!(st.run_pgid, Some(i32::MAX));
    }
    #[test]
    fn panic_releases_the_run_slot_and_wakes_exit() {
        let core = Arc::new(Core::new());
        core.state.lock().unwrap().run = Some("run".into());
        let exiting_core = core.clone();
        let exiting = thread::spawn(move || exit_stop(&exiting_core));
        let owning_core = core.clone();
        let owner = thread::spawn(move || {
            let _slot = RunSlot {
                core: &owning_core,
                id: "run",
                emit: || {},
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
            st.run = Some("run".into());
            st.run_pgid = Some(i32::MAX);
        }
        let (polling_tx, polling_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let owner_core = core.clone();
        let owner = thread::spawn(move || {
            observe_process(
                &owner_core,
                "run",
                || {
                    polling_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(Some(0))
                },
                STOP_GRACE,
                STOP_CONFIRM,
            )
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
        assert!(st.run_pgid.is_none());
        assert!(!st.run_stopping);
    }
    #[test]
    fn app_exit_waits_for_a_reaped_runs_save_and_never_signals_it() {
        let core = Arc::new(Core::new());
        core.state.lock().unwrap().run = Some("run".into());
        let (finish_tx, finish_rx) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let finishing_core = core.clone();
        let finishing = thread::spawn(move || {
            let _slot = RunSlot {
                core: &finishing_core,
                id: "run",
                emit: || {},
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
        assert!(!st.run_stopping);
        assert!(st.run_pgid.is_none());
    }
    #[test]
    fn unconfirmed_kill_expires_without_releasing_a_successors_slot() {
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some("run".into());
            st.run_pgid = Some(i32::MAX);
            begin_stop(&mut st);
        }
        let exit = observe_process(&core, "run", || Ok(None), Duration::ZERO, Duration::ZERO);
        assert_eq!(exit.as_ref().unwrap_err().kind(), io::ErrorKind::TimedOut);
        let mut st = core.state.lock().unwrap();
        assert!(st.run_pgid.is_none());
        assert!(apply_reap(&mut st, "run", &exit));
        assert!(st.run.is_none());
        assert_eq!(st.run_note.as_deref(), Some(STOP_UNCONFIRMED));
        st.run = Some("next".into());
        st.run_pgid = Some(i32::MAX);
        assert!(!apply_reap(&mut st, "run", &exit));
        assert_eq!(st.run.as_deref(), Some("next"));
        assert_eq!(st.run_pgid, Some(i32::MAX));
    }
}
