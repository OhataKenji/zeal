//! One runner-owned session survives through process observation, BES commit
//! completion and artifact cleanup. Invocation IDs come only from Bazel.
use crate::bes::{Context, Session};
use crate::state::{Core, CoreState};
use crate::{profile, store};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;
const STOP_GRACE: Duration = Duration::from_secs(3);
const STOP_CONFIRM: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(25);

pub(crate) fn argv(cmd: &str, target: &str, dir: &Path, port: u16) -> Vec<String> {
    vec![
        cmd.into(),
        target.into(),
        format!("--bes_backend=grpc://127.0.0.1:{port}"),
        "--bes_proxy=".into(),
        "--nobes_lifecycle_events".into(),
        "--bes_upload_mode=wait_for_upload_complete".into(),
        "--bes_timeout=10s".into(),
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
    let (Some(ws), Some(bazel), Some(port)) =
        (state.workspace.clone(), state.bazel.clone(), state.bes_port)
    else {
        return Err("no workspace, bazel or BES".into());
    };
    let ws = ws
        .canonicalize()
        .map_err(|_| "workspace canonicalization failed")?;
    store::ensure_ready(data).map_err(|e| e.to_string())?;
    let profile_dir = store::ProfileDir::new(data).map_err(|e| e.to_string())?;
    let context = Context {
        workspace: ws.clone(),
        cmd: cmd.into(),
        target: target.into(),
    };
    let session = Session::new(context);
    let mut command = Command::new(bazel);
    command
        .args(argv(cmd, target, profile_dir.path(), port))
        .current_dir(&ws)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    // Remove inherited BES/environment proxy destinations. Cache and RBE flags
    // and credentials remain Bazel's responsibility and are not rewritten.
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_lowercase();
        if name.contains("bes_")
            || [
                "http_proxy",
                "https_proxy",
                "all_proxy",
                "grpc_proxy",
                "no_grpc_proxy",
            ]
            .contains(&name.as_str())
        {
            command.env_remove(key);
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            let cleanup = profile_dir.cleanup();
            return Err(if cleanup.is_err() {
                "spawn failed; profile cleanup failed"
            } else {
                "spawn failed"
            }
            .into());
        }
    };
    let id = uuid::Uuid::new_v4().to_string();
    state.run = Some(id.clone());
    state.run_pgid = Some(child.id() as i32);
    state.session = Some(session.clone());
    state.run_stopping = false;
    state.stop_started = None;
    state.run_note = None;
    state.completed_runs.clear();
    state.process_exit = None;
    drop(state);
    core.emit(&app);
    let data = data.to_path_buf();
    let token = id.clone();
    thread::spawn(move || {
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
        let (ids, bes_error) = session.close();
        if let Some(error) = bes_error {
            append_note(&mut note, error);
        }
        if exit.is_some() {
            let attachment = (|| {
                let source = profile_dir
                    .open()
                    .map_err(|_| "profile を読み込めません".to_string())?;
                let id = profile::build_id(source)?;
                let id = uuid::Uuid::parse_str(&id)
                    .map_err(|_| "profile ID が不正です".to_string())?
                    .to_string();
                if !ids.contains(&id) {
                    return Err("profile に対応する保存済み記録がありません".into());
                }
                store::attach_profile(&data, &ws, &id, &profile_dir)
                    .map_err(|_| "profile 添付に失敗しました".into())
            })();
            if let Err(error) = attachment {
                append_note(&mut note, error);
            }
            if profile_dir.cleanup().is_err() {
                append_note(&mut note, "profile 一時 dir の削除に失敗しました".into());
            }
        }
        let mut state = core.state.lock().unwrap();
        if state.run.as_deref() == Some(&token) {
            state.run = None;
            state.run_pgid = None;
            state.session = None;
            state.run_stopping = false;
            state.stop_started = None;
            state.completed_runs = ids;
            state.process_exit = exit;
            state.run_note = note;
        }
        drop(state);
        core.emit(&app);
    });
    Ok(id)
}
/// Own the child observation until confirmed reap or bounded stop give-up.
/// The callback is try_wait, so no blocking wait may hold the signal lock.
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
                    Some("停止を確認できません。一時 profile は残っています".into()),
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
    let session = {
        let mut state = core.state.lock().unwrap();
        begin_stop(&mut state);
        state.session.clone()
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let state = core.state.lock().unwrap();
        if state.run_pgid.is_none() {
            break;
        }
        if Instant::now() >= deadline {
            if let Some(pgid) = state.run_pgid {
                let _ = signal_group(pgid, libc::SIGKILL);
            }
            break;
        }
        drop(state);
        thread::sleep(POLL);
    }
    // Reap clears pgid before artifact cleanup. App exit must still close RPC
    // admission and join any started commit, even during that cleanup window.
    if let Some(session) = session {
        session.close();
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
    fn observation_flags_preserve_bazel_invocation_identity() {
        let args = argv("build", "//...", Path::new("/private/profile"), 1234);
        assert_eq!(args[1], "//...");
        assert!(args.contains(&"--bes_backend=grpc://127.0.0.1:1234".into()));
        assert!(args.contains(&"--bes_proxy=".into()));
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains("invocation_id") || arg.contains("build_event_json"))
        );
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
    fn reap_clears_signal_ownership_while_session_slot_stays_held() {
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
    fn process_output_is_discarded_and_never_depends_on_descendant_eof() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 30 & printf hidden; exit 7"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id() as i32;
        let core = Core::new();
        core.state.lock().unwrap().run_pgid = Some(pgid);
        let (exit, _) = observe_process(
            &core,
            || child.try_wait().map(|s| s.map(|s| s.code().unwrap_or(-1))),
            STOP_GRACE,
            STOP_CONFIRM,
        );
        assert_eq!(exit, Some(7));
        assert!(child.stdout.is_none());
        assert!(child.stderr.is_none());
        let _ = signal_group(pgid, libc::SIGKILL);
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
}
