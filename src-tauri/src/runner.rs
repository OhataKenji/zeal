//! One GUI run owns its child through exit observation, immutable summary save
//! and artifact cleanup. Raw process output goes directly to a private file.
use crate::state::{Core, CoreState};
use crate::store::{self, RunRecord};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
const STOP_GRACE: Duration = Duration::from_secs(3);
const STOP_CONFIRM: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(25);

fn argv(cmd: &str, target: &str, dir: &Path) -> Vec<String> {
    vec![
        cmd.into(),
        target.into(),
        format!("--profile={}", dir.join("profile.gz").display()),
        "--experimental_profile_include_primary_output".into(),
        "--noslim_profile".into(),
        "--tool_tag=zeal".into(),
    ]
}
fn validate(cmd: &str, target: &str) -> Result<(), String> {
    if !["build", "test"].contains(&cmd) {
        return Err("unsupported command".into());
    }
    if target.is_empty()
        || target.starts_with('-')
        || target.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("target must be one label or pattern, without whitespace or control characters, and must not start with '-'".into());
    }
    Ok(())
}
fn now_ms() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "invalid system time")?
        .as_millis();
    let millis = u64::try_from(millis).map_err(|_| "invalid system time")?;
    if millis > 253_402_300_799_999 {
        return Err("invalid system time".into());
    }
    Ok(millis)
}
fn outcome(code: i32, stopped: bool) -> &'static str {
    match code {
        0 => "ok",
        8 => "interrupted",
        _ if stopped => "interrupted",
        _ => "failed",
    }
}
fn spawn_logged(command: &mut Command, dir: &store::ProfileDir) -> Result<Child, String> {
    let stdout = dir.log_output().map_err(|_| "log output unavailable")?;
    let stderr = stdout.try_clone().map_err(|_| "log output unavailable")?;
    command
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0)
        .spawn()
        .map_err(|_| "spawn failed".into())
}
pub fn start(
    app: AppHandle,
    core: Arc<Core>,
    data: &Path,
    cmd: &str,
    target: &str,
) -> Result<String, String> {
    validate(cmd, target)?;
    let mut state = core.state.lock().unwrap();
    if state.ingest_running || state.run.is_some() {
        return Err("bazel is busy".into());
    }
    let (Some(ws), Some(bazel)) = (state.workspace.clone(), state.bazel.clone()) else {
        return Err("no workspace or bazel".into());
    };
    let ws = ws
        .canonicalize()
        .map_err(|_| "workspace canonicalization failed")?;
    store::ensure_ready(data).map_err(|e| e.to_string())?;
    let profile_dir = store::ProfileDir::new(data).map_err(|e| e.to_string())?;
    let started_at = match now_ms() {
        Ok(time) => time,
        Err(error) => {
            let cleanup = profile_dir.cleanup();
            return Err(if cleanup.is_err() {
                format!("{error}; temporary cleanup failed")
            } else {
                error
            });
        }
    };
    let started = Instant::now();
    let id = uuid::Uuid::new_v4().to_string();
    let mut command = Command::new(bazel);
    command
        .args(argv(cmd, target, profile_dir.path()))
        .current_dir(&ws);
    let mut child = match spawn_logged(&mut command, &profile_dir) {
        Ok(child) => child,
        Err(error) => {
            let cleanup = profile_dir.cleanup();
            return Err(if cleanup.is_err() {
                format!("{error}; temporary cleanup failed")
            } else {
                error
            });
        }
    };
    state.run = Some(id.clone());
    state.run_pgid = Some(child.id() as i32);
    state.run_stopping = false;
    state.stop_started = None;
    state.run_note = None;
    state.completed_runs.clear();
    state.process_exit = None;
    drop(state);
    core.emit(&app);
    let data = data.to_path_buf();
    let record = RunRecord {
        id: id.clone(),
        workspace: ws.to_string_lossy().into_owned(),
        workspace_id: String::new(),
        cmd: cmd.into(),
        target: target.into(),
        started_at,
        finished_at: None,
        exit_code: None,
        elapsed_ms: None,
        outcome: String::new(),
        log_truncated: false,
    };
    thread::spawn(move || {
        // Release and wake app exit even if a finishing task panics.
        let slot = RunSlot {
            core: core.clone(),
            id: record.id.clone(),
        };
        let (exit, mut note) = observe_process(
            &core,
            || {
                child
                    .try_wait()
                    .map(|status| status.map(|s| s.code().unwrap_or(-1)))
            },
            STOP_GRACE,
            STOP_CONFIRM,
        );
        let mut ids = Vec::new();
        if let Some(code) = exit {
            let stopped = core.state.lock().unwrap().run_stopping;
            let mut record = record;
            record.finished_at = now_ms().ok();
            record.exit_code = Some(code);
            record.elapsed_ms = u64::try_from(started.elapsed().as_millis()).ok();
            record.outcome = outcome(code, stopped).into();
            let (saved, save_note) = save_completed(&data, &ws, record, profile_dir);
            ids = saved;
            if let Some(error) = save_note {
                append_note(&mut note, error);
            }
        }
        let mut state = core.state.lock().unwrap();
        state.completed_runs = ids;
        state.process_exit = exit;
        state.run_note = note;
        drop(state);
        drop(slot);
        core.emit(&app);
    });
    Ok(id)
}
fn save_completed(
    data: &Path,
    ws: &Path,
    record: RunRecord,
    dir: store::ProfileDir,
) -> (Vec<String>, Option<String>) {
    let mut note = None;
    let cleanup_failed = Arc::new(AtomicBool::new(false));
    let saved = (|| {
        let mut log = dir.capture_log()?;
        log.report_cleanup_to(cleanup_failed.clone());
        store::commit(data, ws, record, log)
    })();
    let ids = match saved {
        Ok(record) => {
            if store::attach_profile(data, ws, &record.id, &dir).is_err() {
                append_note(&mut note, "profile 添付に失敗しました".into());
            }
            vec![record.id]
        }
        Err(error) => {
            append_note(&mut note, format!("実行結果を保存できません: {error}"));
            Vec::new()
        }
    };
    if cleanup_failed.load(Ordering::Acquire) {
        append_note(&mut note, "log 一時 file の削除に失敗しました".into());
    }
    if dir.cleanup().is_err() {
        append_note(&mut note, "実行の一時 dir の削除に失敗しました".into());
    }
    (ids, note)
}
struct RunSlot {
    core: Arc<Core>,
    id: String,
}
impl Drop for RunSlot {
    fn drop(&mut self) {
        let mut state = self
            .core
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.run.as_deref() == Some(&self.id) {
            state.run = None;
            state.run_pgid = None;
            state.run_stopping = false;
            state.stop_started = None;
            if thread::panicking() {
                state.run_note = Some("実行結果の後処理に失敗しました".into());
            }
        }
        self.core.run_finished.notify_all();
    }
}
/// try_wait and signal liveness share one lock; a reaped PID loses ownership
/// before saving begins. Wait errors enter the same bounded stopping path.
fn observe_process(
    core: &Core,
    mut poll: impl FnMut() -> std::io::Result<Option<i32>>,
    grace: Duration,
    confirm: Duration,
) -> (Option<i32>, Option<String>) {
    let mut note = None;
    let mut killed = false;
    loop {
        let mut state = core.state.lock().unwrap();
        match poll() {
            Ok(Some(code)) => {
                state.run_pgid = None;
                return (Some(code), note);
            }
            Ok(None) => (),
            Err(_) => {
                note = Some("process 終了確認に失敗しました".into());
                begin_stop(&mut state);
            }
        }
        if let Some(start) = state.stop_started {
            if start.elapsed() >= grace && !killed {
                if let Some(pgid) = state.run_pgid {
                    let _ = signal_group(pgid, libc::SIGKILL);
                }
                killed = true;
            }
            if start.elapsed() >= grace + confirm {
                state.run_pgid = None;
                return (
                    None,
                    Some("停止を確認できません。実行の一時 dir は残っています".into()),
                );
            }
        }
        drop(state);
        thread::sleep(POLL);
    }
}
fn append_note(note: &mut Option<String>, message: String) {
    match note {
        Some(note) => {
            note.push_str("; ");
            note.push_str(&message);
        }
        None => *note = Some(message),
    }
}
fn begin_stop(state: &mut CoreState) {
    if state.stop_started.is_some() || state.run_pgid.is_none() {
        return;
    }
    state.run_stopping = true;
    state.stop_started = Some(Instant::now());
    if let Some(pgid) = state.run_pgid {
        let _ = signal_group(pgid, libc::SIGINT);
    }
}
pub fn stop(app: AppHandle, core: Arc<Core>) -> Result<(), String> {
    begin_stop(&mut core.state.lock().unwrap());
    core.emit(&app);
    Ok(())
}
pub fn exit_stop(core: &Core) {
    let id = {
        let mut state = core.state.lock().unwrap();
        begin_stop(&mut state);
        state.run.clone()
    };
    let Some(id) = id else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let mut state = core.state.lock().unwrap();
        if state.run.as_deref() != Some(&id) {
            return;
        }
        if state.run_pgid.is_none() {
            while state.run.as_deref() == Some(&id) {
                state = core
                    .run_finished
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
            return;
        }
        if Instant::now() >= deadline {
            if let Some(pgid) = state.run_pgid {
                let _ = signal_group(pgid, libc::SIGKILL);
            }
            return;
        }
        drop(state);
        thread::sleep(POLL);
    }
}
fn signal_group(pgid: i32, signal: i32) -> std::io::Result<()> {
    // SAFETY: kill only delivers a signal; the runner holds the liveness lock.
    if unsafe { libc::kill(-pgid, signal) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_input_is_one_argument() {
        assert!(validate("test", "//a:b$(id)").is_ok());
        assert!(validate("run", "//a:b").is_err());
        assert!(validate("build", "--help").is_err());
        assert!(validate("build", "//a:b c").is_err());
        assert!(validate("build", "//a:\u{9f}").is_err());
    }
    #[test]
    fn flags_only_collect_the_existing_profile() {
        let args = argv("build", "//...", Path::new("/private/profile"));
        assert_eq!(
            args,
            [
                "build",
                "//...",
                "--profile=/private/profile/profile.gz",
                "--experimental_profile_include_primary_output",
                "--noslim_profile",
                "--tool_tag=zeal"
            ]
        );
    }
    #[test]
    fn outcome_describes_the_process_and_requested_stop() {
        assert_eq!(outcome(0, true), "ok");
        assert_eq!(outcome(8, false), "interrupted");
        assert_eq!(outcome(-1, true), "interrupted");
        assert_eq!(outcome(-1, false), "failed");
        assert_eq!(outcome(1, true), "interrupted");
        assert_eq!(outcome(1, false), "failed");
    }
    #[test]
    fn wait_errors_are_bounded_and_release_signal_ownership_before_cleanup() {
        let core = Core::new();
        {
            let mut state = core.state.lock().unwrap();
            state.run = Some("session".into());
            state.run_pgid = Some(i32::MAX);
        }
        let started = Instant::now();
        let (exit, note) = observe_process(
            &core,
            || Err(std::io::Error::from_raw_os_error(libc::ECHILD)),
            Duration::ZERO,
            Duration::ZERO,
        );
        assert_eq!(exit, None);
        assert!(note.unwrap().contains("停止を確認できません"));
        assert!(started.elapsed() < Duration::from_secs(1));
        let mut state = core.state.lock().unwrap();
        assert_eq!(state.run_pgid, None);
        assert_eq!(state.run.as_deref(), Some("session"));
        state.stop_started = None;
        state.run_stopping = false;
        begin_stop(&mut state);
        assert!(!state.run_stopping);
    }
    #[test]
    fn reap_clears_signal_ownership_while_run_slot_stays_held() {
        let core = Core::new();
        {
            let mut state = core.state.lock().unwrap();
            state.run = Some("session".into());
            state.run_pgid = Some(i32::MAX);
        }
        assert_eq!(
            observe_process(&core, || Ok(Some(7)), STOP_GRACE, STOP_CONFIRM),
            (Some(7), None)
        );
        let mut state = core.state.lock().unwrap();
        assert_eq!(state.run_pgid, None);
        assert_eq!(state.run.as_deref(), Some("session"));
        begin_stop(&mut state);
        assert!(!state.run_stopping);
    }
    #[test]
    fn repeated_stop_keeps_the_original_deadline() {
        let core = Core::new();
        let mut state = core.state.lock().unwrap();
        state.run_pgid = Some(i32::MAX);
        begin_stop(&mut state);
        let first = state.stop_started;
        begin_stop(&mut state);
        assert_eq!(state.stop_started, first);
    }
    #[test]
    fn panic_releases_reaped_run_and_wakes_app_exit() {
        let core = Arc::new(Core::new());
        core.state.lock().unwrap().run = Some("run".into());
        let slot = RunSlot {
            core: core.clone(),
            id: "run".into(),
        };
        let waiter_core = core.clone();
        let waiter = thread::spawn(move || exit_stop(&waiter_core));
        let panic = thread::spawn(move || {
            let _slot = slot;
            panic!("injected finishing failure");
        });
        assert!(panic.join().is_err());
        waiter.join().unwrap();
        assert!(core.state.lock().unwrap().run.is_none());
        assert!(
            core.state
                .lock()
                .unwrap()
                .run_note
                .as_ref()
                .unwrap()
                .contains("後処理")
        );
    }
    struct Fixture {
        root: std::path::PathBuf,
        data: std::path::PathBuf,
        ws: std::path::PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-process-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let data = root.join("data");
            let ws = root.join("ws");
            std::fs::create_dir(&ws).unwrap();
            let ws = ws.canonicalize().unwrap();
            store::ensure_ready(&data).unwrap();
            Self { root, data, ws }
        }
        fn record(&self, cmd: &str, target: &str, code: i32) -> RunRecord {
            RunRecord {
                id: uuid::Uuid::new_v4().to_string(),
                workspace: self.ws.to_string_lossy().into_owned(),
                workspace_id: String::new(),
                cmd: cmd.into(),
                target: target.into(),
                started_at: now_ms().unwrap(),
                finished_at: now_ms().ok(),
                exit_code: Some(code),
                elapsed_ms: Some(1),
                outcome: outcome(code, false).into(),
                log_truncated: false,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn completed_child_saves_both_output_streams_and_its_profile() {
        let fixture = Fixture::new();
        let dir = store::ProfileDir::new(&fixture.data).unwrap();
        std::fs::write(
            dir.path().join("profile.gz"),
            b"profile from this GUI operation",
        )
        .unwrap();
        let mut child = spawn_logged(
            Command::new("sh").args(["-c", "printf 'out\r\n'; printf 'err\n' >&2; exit 7"]),
            &dir,
        )
        .unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(7));
        assert!(child.stdout.is_none());
        assert!(child.stderr.is_none());
        let record = fixture.record("build", "//...", status.code().unwrap());
        let id = record.id.clone();
        let path = dir.path().to_path_buf();
        let (ids, note) = save_completed(&fixture.data, &fixture.ws, record, dir);
        assert_eq!(ids.as_slice(), std::slice::from_ref(&id));
        assert_eq!(note, None);
        assert_eq!(
            store::record(&fixture.data, &fixture.ws, &id)
                .unwrap()
                .outcome,
            "failed"
        );
        assert_eq!(
            store::read_log(&fixture.data, &fixture.ws, &id)
                .unwrap()
                .text,
            "out\r\nerr\n"
        );
        assert!(store::open_profile(&fixture.data, &fixture.ws, &id).is_ok());
        assert!(!path.exists());
    }
    #[test]
    fn failed_spawn_creates_no_history_and_temporary_output_can_be_cleaned() {
        let fixture = Fixture::new();
        let dir = store::ProfileDir::new(&fixture.data).unwrap();
        let path = dir.path().to_path_buf();
        assert!(
            spawn_logged(
                &mut Command::new(fixture.root.join("missing-executable")),
                &dir
            )
            .is_err()
        );
        dir.cleanup().unwrap();
        assert!(!path.exists());
        assert!(store::list(&fixture.data, &fixture.ws).unwrap().is_empty());
    }
    #[test]
    #[ignore = "requires cached Bazel 8.7.0; verifies real build and test process history"]
    fn real_bazel_process_history() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture.ws.join("MODULE.bazel"),
            "module(name=\"process_fixture\")\n",
        )
        .unwrap();
        std::fs::write(
            fixture.ws.join("BUILD.bazel"),
            "filegroup(name=\"empty\")\n",
        )
        .unwrap();
        real_bazel_command(&fixture, "build", "//...", 0);
        real_bazel_command(&fixture, "test", "//...", 4);
        assert_eq!(store::list(&fixture.data, &fixture.ws).unwrap().len(), 2);
    }
    fn real_bazel_command(fixture: &Fixture, cmd: &str, target: &str, expected: i32) {
        let dir = store::ProfileDir::new(&fixture.data).unwrap();
        let mut command = Command::new("bazel");
        command
            .env("USE_BAZEL_VERSION", "8.7.0")
            .args(["--batch", "--ignore_all_rc_files"])
            .args(argv(cmd, target, dir.path()))
            .current_dir(&fixture.ws);
        let started = Instant::now();
        let mut child = spawn_logged(&mut command, &dir).unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(expected));
        let mut record = fixture.record(cmd, target, expected);
        record.elapsed_ms = u64::try_from(started.elapsed().as_millis()).ok();
        let id = record.id.clone();
        let (ids, note) = save_completed(&fixture.data, &fixture.ws, record, dir);
        assert_eq!(ids.as_slice(), std::slice::from_ref(&id));
        assert_eq!(note, None);
        assert_eq!(
            store::record(&fixture.data, &fixture.ws, &id)
                .unwrap()
                .exit_code,
            Some(expected)
        );
        assert!(
            !store::read_log(&fixture.data, &fixture.ws, &id)
                .unwrap()
                .text
                .is_empty()
        );
        assert!(
            store::open_profile(&fixture.data, &fixture.ws, &id)
                .unwrap()
                .metadata()
                .unwrap()
                .len()
                > 0
        );
        if expected == 0 {
            let detail = crate::ingest::run_detail(&fixture.data, &fixture.ws, &id).unwrap();
            assert!(matches!(detail.log, crate::ingest::Section::Ok { .. }));
            assert!(matches!(detail.profile, crate::ingest::Section::Ok { .. }));
        }
    }
}
