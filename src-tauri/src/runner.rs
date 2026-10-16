//! Runs `bazel build|test <target>` with the observation flags, streams its
//! output, and records it in the run store.
//!
//! One bazel process at a time: `start` fails while a query or another run is
//! in flight (the UI also disables Send). There is no queue.

use crate::ingest;
use crate::state::Core;
use crate::store::{self, RunRecord};
use serde::Serialize;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use tauri::{AppHandle, Emitter};

/// Payload of the `run-log` event: one line of stdout or stderr.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunLog {
    id: String,
    line: String,
}

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

/// Start a run in the open workspace and return its record. The run finishes
/// in the background; on exit the snapshot is refetched.
pub fn start(
    app: AppHandle,
    core: Arc<Core>,
    data: &Path,
    cmd: &str,
    target: &str,
) -> Result<RunRecord, String> {
    if cmd != "build" && cmd != "test" {
        return Err(format!("unsupported command: {cmd}"));
    }
    let mut st = core.state.lock().unwrap();
    let (Some(ws), Some(bazel)) = (st.workspace.clone(), st.bazel.clone()) else {
        return Err("no workspace or bazel".to_string());
    };
    if st.ingest_running || st.run.is_some() {
        return Err("bazel is busy".to_string());
    }
    let (rec, dir) = store::create(data, &ws, cmd, target).map_err(|e| e.to_string())?;
    // Its own process group, so that M3 can signal bazel and its children.
    let mut child = match Command::new(&bazel)
        .args(argv(cmd, target, &dir))
        .current_dir(&ws)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            return Err(format!("failed to spawn {}: {e}", bazel.display()));
        }
    };
    st.run = Some(rec.id.clone());
    drop(st);
    core.emit(&app);

    // Both pipes feed one channel so that only one thread writes `log.txt`.
    let (tx, rx) = mpsc::channel();
    read_lines(child.stdout.take().expect("stdout piped"), tx.clone());
    read_lines(child.stderr.take().expect("stderr piped"), tx);
    let id = rec.id.clone();
    thread::spawn(move || {
        for line in rx {
            if let Err(e) = store::append_log(&dir, &line) {
                log::warn!("append log failed: {e}");
            }
            let _ = app.emit(
                "run-log",
                RunLog {
                    id: id.clone(),
                    line,
                },
            );
        }
        // -1 when bazel was killed by a signal.
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        if let Err(e) = store::finish(&dir, code) {
            log::warn!("finish run failed: {e}");
        }
        core.state.lock().unwrap().run = None;
        core.emit(&app);
        ingest::request(app, core);
    });
    Ok(rec)
}

fn read_lines(pipe: impl Read + Send + 'static, tx: Sender<String>) {
    thread::spawn(move || {
        for bytes in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
            if tx
                .send(String::from_utf8_lossy(&bytes).into_owned())
                .is_err()
            {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_has_target_and_observation_flags() {
        let dir = Path::new("/data/runs/abc");
        assert_eq!(
            argv("test", "//pkg:t", dir),
            [
                "test",
                "//pkg:t",
                "--build_event_json_file=/data/runs/abc/bep.json",
                "--profile=/data/runs/abc/profile.gz",
                "--experimental_profile_include_primary_output",
                "--noslim_profile",
                "--tool_tag=zeal",
            ]
        );
    }
}
