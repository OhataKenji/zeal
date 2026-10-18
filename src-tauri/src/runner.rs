//! Runs `bazel build|test <target>` and writes stdout/stderr directly to log.txt.
//! The UI reads saved observations after the client exits. No output pipes or
//! log events are held by Zeal. Snapshot refresh is manual.
//!
//! Stop is best-effort: SIGINT, then SIGKILL after a grace period. An unknown
//! exit releases the slot with a note and leaves the record unfinished. Only
//! the currently held run may be signalled; descendants are not supervised.

use crate::state::{Core, CoreState};
use crate::store::{self, RunRecord};
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Commands the composer offers. Zeal assembles every other argument, so a
/// free-form command line never reaches bazel.
const COMMANDS: [&str; 2] = ["build", "test"];

/// Time SIGINT gets to end the run before its group is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// Time SIGKILL gets to end the run before its exit is called unknown and its
/// slot is released.
const STOP_CONFIRM: Duration = Duration::from_secs(2);
/// An app exit cannot defer itself, so its stop is hard-bounded: SIGINT, at
/// most `EXIT_GRACE` of polling, then SIGKILL.
const EXIT_GRACE: Duration = Duration::from_secs(1);
const EXIT_POLL: Duration = Duration::from_millis(25);

/// Shown when the group was killed but no exit was confirmed.
const STOP_UNCONFIRMED: &str = "停止を確認できません";

/// Arguments after the bazel executable. Bazel writes its artifacts into the
/// run dir.
fn argv(cmd: &str, target: &str, dir: &Path) -> Vec<String> {
    vec![
        cmd.to_string(),
        target.to_string(),
        format!("--build_event_json_file={}", dir.join("bep.json").display()),
        format!("--profile={}", dir.join("profile.gz").display()),
        "--experimental_profile_include_primary_output".to_string(),
        "--noslim_profile".to_string(),
        "--tool_tag=zeal".to_string(),
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
    let mut st = core.state.lock().unwrap();
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
    let mut child = match spawn_logged(
        Command::new(&bazel)
            .args(argv(cmd, target, &dir))
            .current_dir(&ws),
        &dir,
    ) {
        Ok(c) => c,
        Err(e) => {
            let _ = store::abort_create(data, &ws, &rec.id);
            return Err(format!("failed to spawn {}: {e}", bazel.display()));
        }
    };
    st.run = Some(rec.id.clone());
    st.run_pgid = Some(child.id() as i32);
    st.run_stopping = false;
    st.run_note = None;
    drop(st);
    core.emit(&app);

    let id = rec.id.clone();
    let data = data.to_path_buf();
    thread::spawn(move || {
        let exit = child.wait().map(|status| status.code().unwrap_or(-1));
        // Finish the record before publishing idle. Holding the state lock
        // prevents a new run or a detail read from observing a half-finished
        // transition. A previously released run stays unknown.
        let mut st = core.state.lock().unwrap();
        if still_held(&st, &id) {
            let saved = match &exit {
                Ok(code) => store::finish(&data, &ws, &id, *code).map(|_| ()),
                Err(_) => Ok(()),
            };
            apply_reap(&mut st, &id, &exit);
            if let Err(e) = saved {
                st.run_note = Some(format!("終了結果を保存できません: {e}"));
            }
        }
        drop(st);
        core.emit(&app);
    });
    Ok(rec)
}

/// Stop the run in flight: SIGINT to its process group, SIGKILL after
/// `STOP_GRACE` (3 s) and, if the exit is still unconfirmed `STOP_CONFIRM`
/// (2 s) later, the slot released with the note. Does nothing when no run is
/// held or a stop is already in flight, so the slot is never held by a stop.
pub fn stop(app: AppHandle, core: Arc<Core>) -> Result<(), String> {
    let (id, pgid) = {
        let mut st = core.state.lock().unwrap();
        if st.run_stopping {
            return Ok(());
        }
        let (Some(id), Some(pgid)) = (st.run.clone(), st.run_pgid) else {
            return Ok(());
        };
        st.run_stopping = true;
        st.run_note = None;
        if let Err(e) = signal_group(pgid, libc::SIGINT) {
            log::warn!("SIGINT to run group failed: {e}");
        }
        (id, pgid)
    };
    core.emit(&app);
    escalate(app, core, id, pgid);
    Ok(())
}

/// The escalation thread of one `stop`: SIGKILL after the grace, and the
/// give-up release and its emit if the exit never came.
fn escalate(app: AppHandle, core: Arc<Core>, id: String, pgid: i32) {
    thread::spawn(move || {
        if escalate_steps(&core, &id, pgid, STOP_GRACE, STOP_CONFIRM) {
            core.emit(&app);
        }
    });
}

/// The escalation sequence: kill the group of the run that is still in flight
/// after `grace` and, if the exit is still unconfirmed `confirm` later, release
/// its slot with the note. Returns whether it gave up; false when the run ended
/// inside the grace. The state lock covers the liveness check and signal,
/// but is never held across a sleep.
fn escalate_steps(core: &Core, id: &str, pgid: i32, grace: Duration, confirm: Duration) -> bool {
    thread::sleep(grace);
    {
        let st = core.state.lock().unwrap();
        if !still_held(&st, id) {
            return false;
        }
        if let Err(e) = signal_group(pgid, libc::SIGKILL) {
            log::warn!("SIGKILL to run group failed: {e}");
        }
    }
    thread::sleep(confirm);
    // Bound first: the lock must not be held into the caller's `emit`.
    give_up(&mut core.state.lock().unwrap(), id)
}

/// Stop the run in flight for an app exit, which cannot wait for a reap: SIGINT
/// to the group, at most `EXIT_GRACE` of polling, SIGKILL if it is still held.
/// The record stays unfinished, as no exit was observed.
pub fn exit_stop(core: &Core) {
    let (id, pgid) = {
        let st = core.state.lock().unwrap();
        let (Some(id), Some(pgid)) = (st.run.clone(), st.run_pgid) else {
            return;
        };
        if let Err(e) = signal_group(pgid, libc::SIGINT) {
            log::warn!("SIGINT to run group failed: {e}");
        }
        (id, pgid)
    };
    if wait_for_release(core, &id, EXIT_GRACE) {
        return;
    }
    let st = core.state.lock().unwrap();
    if still_held(&st, &id)
        && let Err(e) = signal_group(pgid, libc::SIGKILL)
    {
        log::warn!("SIGKILL to run group failed: {e}");
    }
}

/// Wait up to `timeout` for `id` to leave the run slot, polling in `EXIT_POLL`
/// steps. The lock is never held across a sleep.
fn wait_for_release(core: &Core, id: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !still_held(&core.state.lock().unwrap(), id) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
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
    clear_run(st);
    st.run_note = match exit {
        Ok(_) => None,
        Err(e) => Some(format!("終了を確認できません: {e}")),
    };
    true
}

/// Clear the run slot: nothing is in flight, nothing is ours to signal. The
/// note is the caller's: a recorded exit clears it, an unknown one sets it.
fn clear_run(st: &mut CoreState) {
    st.run = None;
    st.run_pgid = None;
    st.run_stopping = false;
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

/// Both streams append to the same file. A descendant retaining its output
/// cannot keep a reader or an EOF wait alive in Zeal.
fn spawn_logged(command: &mut Command, dir: &Path) -> io::Result<Child> {
    let log = fs::OpenOptions::new()
        .append(true)
        .open(dir.join("log.txt"))?;
    let stderr = log.try_clone()?;
    command
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(stderr)
        .process_group(0)
        .spawn()
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
    fn child_writes_both_streams_to_one_log_without_pipe_readers() {
        let dir = LogDir::new();
        let mut child = spawn_logged(
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
        assert_eq!(
            fs::read(dir.0.join("log.txt")).unwrap(),
            b"out\r\nerr\n\xfftail"
        );
        assert_eq!(store::read_log(&dir.0).unwrap(), "out\r\nerr\n\u{fffd}tail");
    }

    #[test]
    fn parent_exit_does_not_wait_for_a_descendant_holding_the_log_open() {
        let dir = LogDir::new();
        let mut child = spawn_logged(
            Command::new("sh")
                .arg("-c")
                .arg("sleep 30 & printf 'done\\n'; exit 7"),
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
        assert_eq!(store::read_log(&dir.0).unwrap(), "done\n");
    }

    #[test]
    fn missing_log_prevents_the_command_from_starting() {
        let dir = LogDir::new();
        fs::remove_file(dir.0.join("log.txt")).unwrap();
        let marker = dir.0.join("spawned");
        let result = spawn_logged(Command::new("touch").arg(&marker), &dir.0);
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
                    "--build_event_json_file=/data/runs/abc/bep.json",
                    "--profile=/data/runs/abc/profile.gz",
                    "--experimental_profile_include_primary_output",
                    "--noslim_profile",
                    "--tool_tag=zeal",
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
    fn command_validation_accepts_only_build_and_test() {
        for cmd in ["build", "test"] {
            assert!(validate_cmd(cmd).is_ok(), "{cmd:?}");
        }
        for cmd in ["run", "clean", "query", ""] {
            assert!(validate_cmd(cmd).is_err(), "{cmd:?}");
        }
    }

    /// The escalation's release: the run leaves the slot with the note that
    /// says the exit is unknown, and the stopping flag goes with it.
    #[test]
    fn give_up_releases_the_run_and_says_the_exit_is_unknown() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some("abc".to_string());
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
        st.run = Some("def".to_string());
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
        st.run = Some("abc".to_string());
        st.run_pgid = Some(42);
        st.run_stopping = true;
        st.run_note = Some("old".to_string());

        assert!(apply_reap(&mut st, "abc", &Ok(3)));
        assert_eq!(st.run, None);
        assert_eq!(st.run_pgid, None);
        assert!(!st.run_stopping);
        assert_eq!(st.run_note, None, "a recorded exit clears the note");

        st.run = Some("abc".to_string());
        st.run_pgid = Some(42);
        let why = std::io::Error::from_raw_os_error(libc::EINTR);
        assert!(apply_reap(&mut st, "abc", &Err(why)));
        assert_eq!(st.run, None, "an unknown exit still releases the slot");
        let note = st.run_note.clone().expect("the reason is shown");
        assert!(note.starts_with("終了を確認できません:"), "{note}");
    }

    /// A reap after the stop gave up: the slot is no longer this run's, so its
    /// record stays unfinished and the give-up note stands. A successor run is
    /// not ours to reap either.
    #[test]
    fn apply_reap_after_give_up_writes_nothing() {
        let core = Core::new();
        let mut st = core.state.lock().unwrap();
        st.run = Some("abc".to_string());
        st.run_pgid = Some(42);
        st.run_stopping = true;
        assert!(give_up(&mut st, "abc"));

        assert!(!apply_reap(&mut st, "abc", &Ok(0)));
        assert_eq!(st.run, None);
        assert_eq!(st.run_note.as_deref(), Some(STOP_UNCONFIRMED), "untouched");

        st.run = Some("def".to_string());
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
            st.run = Some("abc".to_string());
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

    /// The escalation sequence itself: a group that ignores SIGINT is killed
    /// after the grace and, when the exit is still unconfirmed after the
    /// confirm window, its slot is released with the note. The windows are
    /// milliseconds here; `stop` passes `STOP_GRACE` and `STOP_CONFIRM`.
    #[test]
    fn escalate_steps_kill_then_give_up_with_the_note() {
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

        // Exactly what `stop` leaves behind for the escalation.
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some("abc".to_string());
            st.run_pgid = Some(pgid);
            st.run_stopping = true;
        }

        let short = Duration::from_millis(10);
        assert!(escalate_steps(&core, "abc", pgid, short, short));
        {
            let st = core.state.lock().unwrap();
            assert_eq!(st.run, None, "the slot is released");
            assert_eq!(st.run_pgid, None);
            assert!(!st.run_stopping);
            assert_eq!(st.run_note.as_deref(), Some(STOP_UNCONFIRMED));
        }
        // A zombie still holds the group id, so reap the shell before asking
        // whether the group is empty.
        let status = child.wait().unwrap();
        assert!(!status.success(), "unexpected status: {status}");
        let deadline = Instant::now() + Duration::from_secs(10);
        while group_alive(pgid) {
            assert!(Instant::now() < deadline, "group {pgid} still has members");
            thread::sleep(Duration::from_millis(20));
        }

        // The run left the slot inside the grace: nothing to kill, no give-up.
        assert!(!escalate_steps(&core, "abc", pgid, short, short));
    }
}
