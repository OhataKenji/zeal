//! Snapshot ingestion: run `bazel query` once per workspace at a time, parse
//! the XML stream, and swap the in-memory snapshot.
//!
//! Triggers: workspace open and manual refresh. Busy requests are rejected;
//! no pending query or post-run refresh is kept.
//!
//! [`run_detail`] reads a run dir into what the run view shows. It parses the
//! artifacts on every call; nothing is cached.

use crate::profile::{self, Profile};
use crate::snapshot::{self, Status};
use crate::state::{Core, IngestError};
use crate::store::{self, RunRecord};
use crate::xml::{self, RawRule};
use serde::Serialize;
use std::io::{self, BufReader, Read, Write};
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
    pub profile: Section<Profile>,
}

/// Artifact availability never changes an immutable invocation summary.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Section<T> {
    Ok { data: T },
    Incomplete { data: T, note: String },
    Missing,
    ParseError { note: String },
    ReadError { note: String },
}

struct JsonCount {
    bytes: usize,
    limit: usize,
}
impl Write for JsonCount {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| io::Error::other("detail IPC limit exceeded"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// JSON escapes can expand a log sixfold. Stop on a character boundary while
// constructing the IPC result, before allocating an escaped copy.
fn fit_log(text: &mut String, budget: usize) -> bool {
    let mut bytes = 2;
    let mut end = text.len();
    for (index, ch) in text.char_indices() {
        bytes += match ch {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
            '\u{00}'..='\u{1f}' => 6,
            _ => ch.len_utf8(),
        };
        if bytes > budget {
            end = index;
            break;
        }
    }
    let truncated = end < text.len();
    text.truncate(end);
    truncated
}

/// Open only workspace-scoped artifacts through protected storage. One worker
/// owns this call; every call rereads the current files and caches nothing.
pub fn run_detail(data: &Path, workspace: &Path, id: &str) -> Result<RunDetail, String> {
    let record = store::record(data, workspace, id).map_err(|error| error.to_string())?;
    let mut count = JsonCount {
        bytes: 0,
        limit: profile::IPC_LIMIT.saturating_sub(2048),
    };
    serde_json::to_writer(&mut count, &record)
        .map_err(|_| "detail record exceeds IPC limit".to_string())?;
    let mut remaining = profile::IPC_LIMIT - count.bytes - 2048;
    let log = match store::read_log(data, workspace, id) {
        Ok(mut log) => {
            let ipc_truncated = fit_log(&mut log.text, remaining);
            let mut count = JsonCount {
                bytes: 0,
                limit: remaining,
            };
            serde_json::to_writer(&mut count, &log.text)
                .map_err(|_| "detail log exceeds IPC limit".to_string())?;
            remaining -= count.bytes;
            let mut notes = Vec::new();
            if record.log_truncated {
                notes.push("log truncated during save");
            }
            if log.truncated {
                notes.push("log truncated during read");
            }
            if ipc_truncated {
                notes.push("log truncated for IPC limit");
            }
            if notes.is_empty() {
                Section::Ok { data: log.text }
            } else {
                Section::Incomplete {
                    data: log.text,
                    note: notes.join("; "),
                }
            }
        }
        Err(error) => read_error(error),
    };
    let profile = match store::open_profile(data, workspace, id) {
        Err(error) => read_error(error),
        Ok(file)
            if file
                .metadata()
                .is_ok_and(|metadata| metadata.len() > profile::COMPRESSED_LIMIT as u64) =>
        {
            Section::Incomplete {
                data: Profile {
                    phases: vec![],
                    lanes: vec![],
                },
                note: "compressed profile exceeds 32 MiB".to_string(),
            }
        }
        Ok(file) => match profile::parse_bounded(file, remaining) {
            Ok(data) => Section::Ok { data },
            Err(profile::ProfileError::Incomplete(note)) => Section::Incomplete {
                data: Profile {
                    phases: vec![],
                    lanes: vec![],
                },
                note,
            },
            Err(profile::ProfileError::Parse(note)) => Section::ParseError { note },
        },
    };
    Ok(RunDetail {
        record,
        log,
        profile,
    })
}

fn read_error<T>(error: store::StoreError) -> Section<T> {
    if error.kind == store::ErrorKind::Missing {
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
    #[test]
    fn escaped_log_is_limited_at_a_utf8_boundary() {
        let mut text = "\u{0000}é\nhello".to_string();
        assert!(fit_log(&mut text, 10));
        assert_eq!(text, "\u{0000}é");
        assert_eq!(serde_json::to_vec(&text).unwrap().len(), 10);
    }
    #[test]
    fn exact_log_budget_keeps_all_text() {
        let mut text = "é\n".to_string();
        assert!(!fit_log(&mut text, 6));
        assert_eq!(text, "é\n");
    }
    struct Saved {
        root: std::path::PathBuf,
        data: std::path::PathBuf,
        workspace: std::path::PathBuf,
        record: RunRecord,
    }
    impl Drop for Saved {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn saved(log: &str) -> Saved {
        let root = std::env::temp_dir().join(format!("zeal-detail-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let data = root.join("data");
        std::fs::create_dir_all(&workspace).unwrap();
        let mut prepared = store::PreparedLog::new(&data).unwrap();
        prepared.append(log).unwrap();
        let record = store::commit(
            &data,
            &workspace,
            RunRecord {
                id: uuid::Uuid::new_v4().to_string(),
                workspace: String::new(),
                workspace_id: String::new(),
                cmd: "build".to_string(),
                target: "//:app".to_string(),
                started_at: 1,
                finished_at: None,
                exit_code: Some(0),
                bazel_elapsed_ms: None,
                outcome: String::new(),
                log_truncated: false,
            },
            prepared,
        )
        .unwrap();
        Saved {
            root,
            data,
            workspace,
            record,
        }
    }
    fn artifacts(saved: &Saved) -> std::path::PathBuf {
        saved
            .data
            .join("workspaces")
            .join(&saved.record.workspace_id)
            .join("runs")
            .join(&saved.record.id)
    }
    #[test]
    fn missing_files_preserve_finished_result_without_finish_timestamp() {
        let saved = saved("");
        std::fs::remove_file(artifacts(&saved).join("log.txt")).unwrap();
        let detail = run_detail(&saved.data, &saved.workspace, &saved.record.id).unwrap();
        assert_eq!(detail.record, saved.record);
        assert_eq!(detail.record.outcome, "ok");
        assert!(matches!(detail.log, Section::Missing));
        assert!(matches!(detail.profile, Section::Missing));
        assert_eq!(store::list(&saved.data, &saved.workspace).unwrap().len(), 1);
    }
    #[test]
    fn unsafe_log_and_corrupt_profile_are_independent() {
        let saved = saved("text");
        let dir = artifacts(&saved);
        std::fs::remove_file(dir.join("log.txt")).unwrap();
        std::fs::create_dir(dir.join("log.txt")).unwrap();
        std::fs::write(dir.join("profile.gz"), b"not gzip").unwrap();
        std::fs::set_permissions(
            dir.join("profile.gz"),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        let detail = run_detail(&saved.data, &saved.workspace, &saved.record.id).unwrap();
        assert_eq!(detail.record, saved.record);
        assert!(matches!(detail.log, Section::ReadError { .. }));
        assert!(matches!(detail.profile, Section::ParseError { .. }));
    }
    #[test]
    fn ipc_limit_applies_to_escaped_log_during_construction() {
        let saved = saved(&"\u{0000}".repeat(store::MAX_LOG_BYTES));
        let detail = run_detail(&saved.data, &saved.workspace, &saved.record.id).unwrap();
        assert!(matches!(&detail.log, Section::Incomplete { note, .. } if note.contains("IPC")));
        assert!(serde_json::to_vec(&detail).unwrap().len() <= profile::IPC_LIMIT);
        assert_eq!(detail.record, saved.record);
    }
}
