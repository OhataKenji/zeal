//! Write build/test output directly to log.txt and read it after the client exits.

use crate::state::Core;
use crate::store::{self, RunRecord};
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
#[cfg(test)]
use std::time::Duration;
use tauri::AppHandle;

const COMMANDS: [&str; 2] = ["build", "test"];

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
            let _ = fs::remove_dir_all(&dir);
            return Err(format!("failed to spawn {}: {e}", bazel.display()));
        }
    };
    st.run = Some(rec.id.clone());
    st.run_note = None;
    drop(st);
    core.emit(&app);

    let id = rec.id.clone();
    thread::spawn(move || {
        let exit = child.wait().map(|status| status.code().unwrap_or(-1));
        // Finish the record before publishing idle. Holding the state lock
        // prevents a new run or a detail read from observing a half-finished
        // transition.
        let mut st = core.state.lock().unwrap();
        if st.run.as_deref() == Some(&id) {
            let saved = match &exit {
                Ok(code) => store::finish(&dir, *code).map(|_| ()),
                Err(_) => Ok(()),
            };
            st.run = None;
            st.run_note = exit
                .as_ref()
                .err()
                .map(|e| format!("終了を確認できません: {e}"));
            if let Err(e) = saved {
                st.run_note = Some(format!("終了結果を保存できません: {e}"));
            }
        }
        drop(st);
        core.emit(&app);
    });
    Ok(rec)
}

#[cfg(test)]
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
}
