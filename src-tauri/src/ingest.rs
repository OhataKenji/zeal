//! Snapshot ingestion: run `bazel query` once per workspace at a time, parse
//! the XML stream, and swap the in-memory snapshot.
//!
//! Triggers: workspace open and manual refresh. Busy requests are rejected;
//! no pending query or post-run refresh is kept.

use crate::snapshot::{self, Status};
use crate::state::{Core, IngestError};
use crate::xml::{self, RawRule};
use std::io::{BufReader, Read};
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
