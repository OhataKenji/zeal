//! Snapshot ingestion: run `bazel query` once per workspace at a time, parse
//! the XML stream, and swap the in-memory snapshot.
//!
//! Triggers: workspace open and manual refresh. Busy requests are rejected;
//! no pending query or post-run refresh is kept.
//!
//! Saved run observations: [`run_detail`] returns the record, log and build
//! results; [`run_profile`] returns the timing profile and its availability.

use crate::bep::{self, Bep};
use crate::profile::{self, Profile};
use crate::snapshot::{self, Status};
use crate::state::{Core, IngestError};
use crate::store::{self, RunRecord};
use crate::xml::{self, RawRule};
use serde::Serialize;
use std::io::{self, BufReader, Read};
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
        let mut st = core.state.lock().expect("application state lock poisoned");
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
        let mut st = core.state.lock().expect("application state lock poisoned");
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
}

/// Read the record and available observations from a run dir. A missing or
/// unreadable observation never hides the record or another observation.
#[tracing::instrument(target = "zeal::operations", name = "run.detail", skip_all)]
pub fn run_detail(data: &Path, workspace: &Path, id: &str) -> io::Result<RunDetail> {
    let record = store::read_record(data, workspace, id)?;
    let (log, bep) = match store::event_artifact(data, workspace, id).map_err(io::Error::from) {
        Err(error) => {
            let kind = error.kind();
            let note = error.to_string();
            if kind == io::ErrorKind::NotFound {
                (Section::Missing, Section::Missing)
            } else {
                (
                    Section::ReadError { note: note.clone() },
                    Section::ReadError { note },
                )
            }
        }
        Ok((file, complete)) => {
            let length = file.metadata()?.len();
            let parsed = bep::parse_detail(BufReader::new(file), length);
            if parsed.invalid && parsed.events == 0 {
                let note = "BEP contains an invalid or incomplete event".to_string();
                if parsed.read_error {
                    (
                        Section::ReadError { note: note.clone() },
                        Section::ReadError { note },
                    )
                } else {
                    (
                        Section::ParseError { note: note.clone() },
                        Section::ParseError { note },
                    )
                }
            } else {
                let log = if parsed.invalid || !complete {
                    Section::Incomplete {
                        data: parsed.log,
                        note: "BEP ended with an invalid or incomplete event".into(),
                    }
                } else {
                    Section::Ok { data: parsed.log }
                };
                let bep = if parsed.bep.truncated || !complete {
                    Section::Incomplete {
                        data: parsed.bep,
                        note: "BEP is incomplete or reached its display limit".into(),
                    }
                } else {
                    Section::Ok { data: parsed.bep }
                };
                (log, bep)
            }
        }
    };
    Ok(RunDetail { record, log, bep })
}

/// Read a saved profile, reporting artifact failures as section statuses.
/// Optional artifacts are independent of invocation result and origin.
pub fn run_profile(data: &Path, workspace: &Path, id: &str) -> io::Result<Section<Profile>> {
    store::read_record(data, workspace, id)?;
    Ok({
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
    use crate::store::events::test_support::Fixture;
    use std::fs;
    #[test]
    fn event_details_are_shared_and_missing_files_do_not_hide_build_result() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        let detail = run_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(detail.record.exit_code, Some(0));
        assert!(matches!(detail.log, Section::Ok { .. }));
        assert!(matches!(detail.bep, Section::Ok { .. }));
        assert!(matches!(
            run_profile(&f.data, &f.ws, id).unwrap(),
            Section::Missing
        ));
        fs::remove_file(f.dir().join("attempt/1/bep.binpb")).unwrap();
        let missing = run_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(missing.record, detail.record);
        assert!(matches!(missing.log, Section::Missing));
        assert!(matches!(missing.bep, Section::Missing));
    }
    #[test]
    fn malformed_detail_and_optional_profile_are_independent_sections() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        fs::write(f.dir().join("attempt/1/bep.binpb"), b"\xff").unwrap();
        fs::write(f.dir().join("profile.gz"), b"invalid gzip").unwrap();
        let detail = run_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(detail.record.exit_code, Some(0));
        assert!(matches!(detail.log, Section::ParseError { .. }));
        assert!(matches!(detail.bep, Section::ParseError { .. }));
        assert!(matches!(
            run_profile(&f.data, &f.ws, id).unwrap(),
            Section::ParseError { .. }
        ));
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        assert!(run_detail(&f.data, &other, id).is_err());
        assert!(run_profile(&f.data, &other, id).is_err());
    }

    #[test]
    fn valid_prefix_from_a_disconnected_receipt_is_shown_as_incomplete() {
        let f = Fixture::new();
        let mut receipt = store::events::Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        receipt.partial().unwrap();
        let detail = run_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(matches!(detail.log, Section::Incomplete { .. }));
        assert!(matches!(detail.bep, Section::Incomplete { .. }));
        assert_eq!(detail.record.exit_code, None);
    }
}
