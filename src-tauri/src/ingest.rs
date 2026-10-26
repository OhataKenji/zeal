//! Snapshot ingestion: run `bazel query` once per workspace at a time, parse
//! the XML stream, and swap the in-memory snapshot.
//!
//! Triggers: workspace open and manual refresh. Busy requests are rejected;
//! no pending query or post-run refresh is kept.
//!
//! [`run_detail`] reads the record, log and results. [`run_profile`] parses
//! timing separately so log display never waits for profile parsing.

use crate::bep::{self, Bep};
use crate::profile::{self, Profile};
use crate::snapshot::{self, Status};
use crate::state::{Core, IngestError};
use crate::store::{self, RunRecord};
use crate::xml::{self, RawRule};
use serde::Serialize;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Upper bound for one `bazel query`. First runs may include a bazelisk
/// download and a cold analysis cache.
const QUERY_TIMEOUT: Duration = Duration::from_secs(300);

enum Outcome {
    Ok(Vec<RawRule>),
    /// exit != 0 but at least one rule was parsed.
    Partial(Vec<RawRule>, IngestError),
    Failed(IngestError),
}

/// Start one ingestion. Busy requests are rejected rather than queued.
pub fn request(app: AppHandle, core: Arc<Core>) -> Result<(), String> {
    let (ws, bazel) = {
        let mut st = core.state.lock().unwrap();
        if st.ingest_running || st.run.is_some() {
            return Err("bazel is busy; refresh after it finishes".to_string());
        }
        let (Some(ws), Some(bazel)) = (st.workspace.clone(), st.bazel.clone()) else {
            return Ok(());
        };
        st.ingest_running = true;
        (ws, bazel)
    };
    core.emit(&app);
    thread::spawn(move || {
        let outcome = fetch(&ws, &bazel);
        let snapshot = match &outcome {
            Outcome::Ok(rules) => Some(Arc::new(snapshot::build(rules, Status::Ok))),
            Outcome::Partial(rules, _) => Some(Arc::new(snapshot::build(rules, Status::Partial))),
            Outcome::Failed(_) => None,
        };
        let mut st = core.state.lock().unwrap();
        if let Some(snapshot) = snapshot {
            st.snapshot = Some(snapshot);
        }
        st.last_error = match outcome {
            Outcome::Ok(_) => None,
            Outcome::Partial(_, err) | Outcome::Failed(err) => Some(err),
        };
        st.ingest_running = false;
        drop(st);
        core.emit(&app);
    });
    Ok(())
}

/// Run `bazel query 'kind("rule", //...)' --output=xml --noimplicit_deps
/// --keep_going` in `ws`, streaming stdout through the XML parser.
#[tracing::instrument(target = "zeal::operations", name = "bazel.query", skip_all)]
fn fetch(ws: &Path, bazel: &Path) -> Outcome {
    let mut child = match Command::new(bazel)
        .arg("query")
        .arg("kind(\"rule\", //...)")
        .args(["--output=xml", "--noimplicit_deps", "--keep_going"])
        .current_dir(ws)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return Outcome::Failed(IngestError {
                exit_code: None,
                summary: format!("failed to spawn {}: {e}", bazel.display()),
            });
        }
    };
    // stdout is parsed as it arrives; stderr is drained concurrently so the
    // child never blocks on a full pipe.
    let stdout = child.stdout.take().expect("stdout piped");
    let parse_handle = thread::spawn(move || xml::parse_rules(BufReader::new(stdout)));
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + QUERY_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = parse_handle.join();
                let _ = stderr_handle.join();
                return Outcome::Failed(IngestError {
                    exit_code: None,
                    summary: format!("query timed out after {}s", QUERY_TIMEOUT.as_secs()),
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = parse_handle.join();
                let _ = stderr_handle.join();
                return Outcome::Failed(IngestError {
                    exit_code: None,
                    summary: format!("failed to wait on query: {e}"),
                });
            }
        }
    };

    let rules = parse_handle.join().unwrap_or_default();
    let stderr_bytes = stderr_handle.join().unwrap_or_default();
    let stderr_tail =
        String::from_utf8_lossy(&stderr_bytes[stderr_bytes.len().saturating_sub(8192)..]);
    let code = status.code();
    match code {
        Some(0) => Outcome::Ok(rules),
        _ if !rules.is_empty() => Outcome::Partial(
            rules,
            IngestError {
                exit_code: code,
                summary: summarize(&stderr_tail),
            },
        ),
        _ => Outcome::Failed(IngestError {
            exit_code: code,
            summary: summarize(&stderr_tail),
        }),
    }
}

/// Last non-empty stderr lines, truncated; bazel writes its errors there.
fn summarize(stderr_tail: &str) -> String {
    let lines: Vec<&str> = stderr_tail
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let s = lines[lines.len().saturating_sub(3)..].join(" | ");
    let s = if s.is_empty() {
        "query failed".to_string()
    } else {
        s
    };
    s.chars().take(300).collect()
}

#[derive(Debug, Serialize)]
pub struct RunDetail {
    pub record: RunRecord,
    pub log: Section<String>,
    pub bep: Section<Bep>,
}

/// Observation availability is independent of the process exit code.
/// Only complete and partial observations carry data.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Section<T> {
    Ok { data: T },
    Incomplete { data: T, note: String },
    Missing,
    ParseError { note: String },
    ReadError { note: String },
    NotAvailable { note: String },
}

/// Read the record and available observations from a run dir. A missing or
/// unreadable observation never hides the record or another observation.
#[tracing::instrument(target = "zeal::operations", name = "run.detail", skip_all)]
pub fn run_detail(data: &Path, workspace: &Path, id: &str) -> io::Result<RunDetail> {
    let record = store::read_record(data, workspace, id)?;
    if record.external {
        let observed = store::events::get(data, workspace, id)?
            .ok_or_else(|| io::Error::other("missing BES record"))?;
        let bep = if observed.bep.truncated {
            Section::Incomplete {
                data: observed.bep,
                note: "BES result display reached its 8 MiB limit.".into(),
            }
        } else {
            Section::Ok { data: observed.bep }
        };
        return Ok(RunDetail {
            record,
            log: Section::Ok { data: observed.log },
            bep,
        });
    }
    let log = match store::artifact(data, workspace, id, "log.txt").and_then(|mut file| {
        let start = file.metadata()?.len().saturating_sub(512 * 1024);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if start > 0 {
            let first = bytes
                .iter()
                .position(|b| b & 0xc0 != 0x80)
                .unwrap_or(bytes.len());
            bytes.drain(..first);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }) {
        Ok(data) => Section::Ok { data },
        Err(e) => read_error(e),
    };
    let bep = match store::artifact(data, workspace, id, "bep.json") {
        Err(e) => read_error(e),
        Ok(f) => {
            let data = bep::parse(BufReader::new(f));
            if data.truncated {
                Section::Incomplete {
                    data,
                    note: "BEP ended with an invalid or unreadable event".to_string(),
                }
            } else {
                Section::Ok { data }
            }
        }
    };
    Ok(RunDetail { record, log, bep })
}

/// Timing is read independently and only after the record confirms completion.
pub fn run_profile(data: &Path, workspace: &Path, id: &str) -> io::Result<Section<Profile>> {
    let record = store::read_record(data, workspace, id)?;
    Ok(if record.external {
        Section::NotAvailable {
            note: "Profiles from terminal builds are not imported.".into(),
        }
    } else if record.finished_at.is_none() {
        Section::NotAvailable {
            note: "timing is available after run completion is recorded".to_string(),
        }
    } else {
        match store::artifact(data, workspace, id, "profile.gz") {
            Err(e) => read_error(e),
            Ok(f) => match profile::parse_bounded(f, profile::IPC_LIMIT) {
                Ok(data) => Section::Ok { data },
                Err(profile::ProfileError::Incomplete(note)) => Section::Incomplete {
                    data: Profile {
                        phases: Vec::new(),
                        lanes: Vec::new(),
                    },
                    note,
                },
                Err(profile::ProfileError::Parse(note)) => Section::ParseError { note },
            },
        }
    })
}

fn read_error<T>(error: io::Error) -> Section<T> {
    if error.kind() == io::ErrorKind::NotFound {
        Section::Missing
    } else {
        Section::ReadError {
            note: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Removes the temp root when the test ends.
    struct TempRoot(PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A new unfinished run with a fixture's observations, if present.
    fn run_dir(case: &str) -> (TempRoot, PathBuf) {
        let root = std::env::temp_dir().join(format!("zeal-run-detail-{}", uuid::Uuid::new_v4()));
        let ws = root.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let (_, dir) = store::create(&root.join("data"), &ws, "build", "//...").unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/run");
        for name in ["bep.json", "profile.gz"] {
            let src = fixtures.join(case).join(name);
            if src.exists() {
                fs::write(dir.join(name), fs::read(src).unwrap()).unwrap();
            }
        }
        (TempRoot(root), dir)
    }

    fn detail(root: &TempRoot, dir: &Path) -> io::Result<RunDetail> {
        run_detail(
            &root.0.join("data"),
            &root.0.join("ws"),
            dir.file_name().unwrap().to_str().unwrap(),
        )
    }

    fn timing(root: &TempRoot, dir: &Path) -> io::Result<Section<Profile>> {
        run_profile(
            &root.0.join("data"),
            &root.0.join("ws"),
            dir.file_name().unwrap().to_str().unwrap(),
        )
    }

    fn finish(root: &TempRoot, dir: &Path, code: i32) -> Result<RunRecord, store::StoreError> {
        store::finish(
            &root.0.join("data"),
            &root.0.join("ws"),
            dir.file_name().unwrap().to_str().unwrap(),
            code,
        )
    }

    #[test]
    fn saved_log_reads_only_the_last_512_kib() {
        let (root, dir) = run_dir("ok");
        fs::write(
            dir.join("log.txt"),
            format!("{}tail", "a".repeat(600 * 1024)),
        )
        .unwrap();
        let Section::Ok { data } = detail(&root, &dir).unwrap().log else {
            panic!("expected log");
        };
        assert_eq!(data.len(), 512 * 1024);
        assert!(data.ends_with("tail"));
    }

    #[test]
    fn record_and_log() {
        let (_root, dir) = run_dir("ok");
        store::append_log(&dir, "INFO: Build completed").unwrap();
        let rec = finish(&_root, &dir, 0).unwrap();
        let d = detail(&_root, &dir).unwrap();
        assert_eq!(d.record, rec);
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["record"]["exitCode"], 0);
        assert_eq!(json["log"]["status"], "ok");
        assert_eq!(json["log"]["data"], "INFO: Build completed\n");
        assert!(json.get("profile").is_none());
        assert!(matches!(timing(&_root, &dir).unwrap(), Section::Ok { .. }));
        assert!(matches!(d.log, Section::Ok { data } if data == "INFO: Build completed\n"));
    }

    #[test]
    fn complete_cases_have_both_sections() {
        for case in [
            "ok",
            "disk_cache_hit",
            "action_failed",
            "analysis_failed",
            "loading_failed",
        ] {
            let (_root, dir) = run_dir(case);
            finish(&_root, &dir, 0).unwrap();
            let d = detail(&_root, &dir).unwrap();
            assert!(matches!(d.bep, Section::Ok { .. }), "{case}");
            assert!(
                matches!(timing(&_root, &dir).unwrap(), Section::Ok { .. }),
                "{case}"
            );
        }
    }

    #[test]
    fn incomplete_bep_and_missing_profile_preserve_the_exit_result() {
        let (_root, dir) = run_dir("bep_truncated");
        finish(&_root, &dir, 1).unwrap();
        let d = detail(&_root, &dir).unwrap();
        assert_eq!(d.record.exit_code, Some(1));
        assert!(matches!(d.bep, Section::Incomplete { data, note }
            if !data.targets.is_empty() && !note.is_empty()));
        assert!(matches!(timing(&_root, &dir).unwrap(), Section::Missing));
    }

    #[test]
    fn complete_observations_do_not_imply_process_success() {
        let (_root, dir) = run_dir("ok");
        finish(&_root, &dir, 42).unwrap();
        let d = detail(&_root, &dir).unwrap();
        assert_eq!(d.record.exit_code, Some(42));
        assert!(matches!(d.bep, Section::Ok { .. }));
        assert!(matches!(timing(&_root, &dir).unwrap(), Section::Ok { .. }));
    }

    #[test]
    fn unfinished_profile_is_not_parsed_even_when_it_is_valid() {
        let (_root, dir) = run_dir("ok");
        let d = detail(&_root, &dir).unwrap();
        assert!(matches!(
            timing(&_root, &dir).unwrap(),
            Section::NotAvailable { .. }
        ));
        assert_eq!(d.record.finished_at, None);
        assert_eq!(d.record.exit_code, None);

        finish(&_root, &dir, 0).unwrap();
        assert!(matches!(timing(&_root, &dir).unwrap(), Section::Ok { .. }));
    }

    #[test]
    fn broken_profile_is_reported_only_after_completion_and_preserves_success() {
        let (_root, dir) = run_dir("ok");
        fs::write(dir.join("profile.gz"), b"not gzip").unwrap();
        assert!(matches!(
            timing(&_root, &dir).unwrap(),
            Section::NotAvailable { .. }
        ));

        finish(&_root, &dir, 0).unwrap();
        let d = detail(&_root, &dir).unwrap();
        assert_eq!(d.record.exit_code, Some(0));
        assert!(
            matches!(timing(&_root, &dir).unwrap(), Section::ParseError { note } if !note.is_empty())
        );
        assert!(matches!(d.bep, Section::Ok { .. }));
    }

    #[test]
    fn missing_observations_do_not_hide_the_record() {
        let (_root, dir) = run_dir("ok");
        let rec = finish(&_root, &dir, 0).unwrap();
        fs::remove_file(dir.join("log.txt")).unwrap();
        fs::remove_file(dir.join("bep.json")).unwrap();
        fs::remove_file(dir.join("profile.gz")).unwrap();
        let d = detail(&_root, &dir).unwrap();
        assert_eq!(d.record, rec);
        assert!(matches!(d.log, Section::Missing));
        assert!(matches!(d.bep, Section::Missing));
        assert!(matches!(timing(&_root, &dir).unwrap(), Section::Missing));
    }

    #[test]
    fn unreadable_log_is_reported_without_hiding_other_observations() {
        let (_root, dir) = run_dir("ok");
        finish(&_root, &dir, 0).unwrap();
        fs::remove_file(dir.join("log.txt")).unwrap();
        fs::create_dir(dir.join("log.txt")).unwrap();
        let d = detail(&_root, &dir).unwrap();
        assert!(matches!(d.log, Section::ReadError { note } if !note.is_empty()));
        assert!(matches!(d.bep, Section::Ok { .. }));
        assert!(matches!(timing(&_root, &dir).unwrap(), Section::Ok { .. }));
    }

    #[test]
    fn each_request_reads_the_current_artifacts() {
        let (_root, dir) = run_dir("ok");
        finish(&_root, &dir, 0).unwrap();
        assert!(matches!(timing(&_root, &dir).unwrap(), Section::Ok { .. }));
        fs::remove_file(dir.join("profile.gz")).unwrap();
        assert!(matches!(timing(&_root, &dir).unwrap(), Section::Missing));
    }

    #[test]
    fn a_different_workspace_cannot_read_the_record_or_artifacts() {
        let (root, dir) = run_dir("ok");
        let other = root.0.join("other");
        fs::create_dir(&other).unwrap();
        assert!(
            run_detail(
                &root.0.join("data"),
                &other,
                dir.file_name().unwrap().to_str().unwrap()
            )
            .is_err()
        );
        assert!(
            run_profile(
                &root.0.join("data"),
                &other,
                dir.file_name().unwrap().to_str().unwrap()
            )
            .is_err()
        );
    }
}
