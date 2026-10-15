//! Snapshot ingestion: run `bazel query` once per workspace at a time, parse
//! the XML stream, and swap the in-memory snapshot.
//!
//! Triggers: workspace open (background), manual refresh, and (per M3) after
//! a run completes. Requests arriving while an ingestion runs are recorded as
//! pending and collapse into a single follow-up run; the new snapshot only
//! replaces the old one once it is fully built, so a failed or killed fetch
//! leaves the previous snapshot in place.

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

/// Request an ingestion. No-op without an open workspace and a resolved bazel.
pub fn request(app: AppHandle, core: Arc<Core>) {
    {
        let mut st = core.state.lock().unwrap();
        if st.workspace.is_none() || st.bazel.is_none() {
            return;
        }
        if st.ingest_running {
            st.ingest_pending = true;
            drop(st);
            core.emit(&app);
            return;
        }
        st.ingest_running = true;
    }
    core.emit(&app);
    thread::spawn(move || {
        loop {
            let (ws, bazel) = {
                let st = core.state.lock().unwrap();
                (
                    st.workspace.clone().expect("ingest without workspace"),
                    st.bazel.clone().expect("ingest without bazel"),
                )
            };
            let outcome = fetch(&ws, &bazel);
            let mut st = core.state.lock().unwrap();
            match outcome {
                Outcome::Ok(rules) => {
                    log::info!("ingest: {} rules", rules.len());
                    st.snapshot = Some(Arc::new(snapshot::build(&rules, Status::Ok)));
                    st.last_error = None;
                }
                Outcome::Partial(rules, err) => {
                    log::warn!("ingest partial: {} rules; {}", rules.len(), err.summary);
                    st.snapshot = Some(Arc::new(snapshot::build(&rules, Status::Partial)));
                    st.last_error = Some(err);
                }
                Outcome::Failed(err) => {
                    log::warn!("ingest failed: {}", err.summary);
                    // The previous snapshot stays.
                    st.last_error = Some(err);
                }
            }
            if st.ingest_pending {
                st.ingest_pending = false;
                drop(st);
                core.emit(&app);
                continue;
            }
            st.ingest_running = false;
            drop(st);
            core.emit(&app);
            break;
        }
    });
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
