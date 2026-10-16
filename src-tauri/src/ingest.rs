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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_keeps_last_three_non_empty_lines() {
        assert_eq!(
            summarize("Loading\nERROR: a\n\n  ERROR: b  \nERROR: c\n"),
            "ERROR: a | ERROR: b | ERROR: c"
        );
        assert_eq!(summarize("only\n"), "only");
    }

    #[test]
    fn summarize_falls_back_when_empty() {
        assert_eq!(summarize(""), "query failed");
        assert_eq!(summarize(" \n\n\t\n"), "query failed");
    }

    #[test]
    fn summarize_truncates_to_300_chars() {
        assert_eq!(summarize(&"x".repeat(301)), "x".repeat(300));
        assert_eq!(summarize(&"x".repeat(300)), "x".repeat(300));
        // Counted in chars, not bytes.
        assert_eq!(summarize(&"あ".repeat(400)), "あ".repeat(300));
    }

    #[cfg(unix)]
    mod fake_bazel {
        use super::super::*;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/query.xml");

        /// Writing an executable while another test thread forks can make
        /// exec fail with ETXTBSY, so script creation and spawn are serialized.
        static SPAWN_LOCK: Mutex<()> = Mutex::new(());

        struct TempDir(PathBuf);

        impl TempDir {
            fn new() -> Self {
                static N: AtomicUsize = AtomicUsize::new(0);
                let dir = std::env::temp_dir().join(format!(
                    "zeal-ingest-test-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                let _ = fs::remove_dir_all(&dir);
                fs::create_dir_all(&dir).unwrap();
                TempDir(dir)
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        /// Run `fetch` with a `#!/bin/sh` fake bazel whose body is `body`.
        fn run(body: &str) -> (TempDir, Outcome) {
            let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = TempDir::new();
            let bazel = dir.0.join("bazel");
            fs::write(&bazel, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&bazel, fs::Permissions::from_mode(0o755)).unwrap();
            let outcome = fetch(&dir.0, &bazel);
            (dir, outcome)
        }

        fn fixture_rule_count() -> usize {
            fs::read_to_string(FIXTURE)
                .unwrap()
                .matches("<rule ")
                .count()
        }

        #[test]
        fn exit_zero_is_ok_with_all_rules() {
            let (dir, outcome) = run(&format!(
                "printf '%s\\n' \"$@\" > args.txt\ncat '{FIXTURE}'\nexit 0"
            ));
            let Outcome::Ok(rules) = outcome else {
                panic!("expected Ok");
            };
            assert_eq!(rules.len(), fixture_rule_count());
            assert!(rules.iter().any(|r| r.label == "//app/lib:parser"));
            // Arguments and working directory reach the child.
            assert_eq!(
                fs::read_to_string(dir.0.join("args.txt")).unwrap(),
                "query\nkind(\"rule\", //...)\n--output=xml\n--noimplicit_deps\n--keep_going\n"
            );
        }

        #[test]
        fn nonzero_exit_with_rules_is_partial() {
            let (_dir, outcome) = run(&format!(
                "cat '{FIXTURE}'\n\
                 printf 'Loading\\nERROR: a\\n\\n  ERROR: b  \\nERROR: c\\n' >&2\n\
                 exit 3"
            ));
            let Outcome::Partial(rules, err) = outcome else {
                panic!("expected Partial");
            };
            assert_eq!(rules.len(), fixture_rule_count());
            assert_eq!(err.exit_code, Some(3));
            assert_eq!(err.summary, "ERROR: a | ERROR: b | ERROR: c");
        }

        #[test]
        fn nonzero_exit_without_rules_is_failed() {
            let (_dir, outcome) = run("echo 'ERROR: no such package' >&2\nexit 7");
            let Outcome::Failed(err) = outcome else {
                panic!("expected Failed");
            };
            assert_eq!(err.exit_code, Some(7));
            assert_eq!(err.summary, "ERROR: no such package");
        }

        #[test]
        fn nonzero_exit_without_stderr_uses_fallback_summary() {
            let (_dir, outcome) = run("exit 1");
            let Outcome::Failed(err) = outcome else {
                panic!("expected Failed");
            };
            assert_eq!(err.exit_code, Some(1));
            assert_eq!(err.summary, "query failed");
        }

        #[test]
        fn killed_by_signal_is_failed_without_exit_code() {
            let (_dir, outcome) = run("kill -9 $$");
            let Outcome::Failed(err) = outcome else {
                panic!("expected Failed");
            };
            assert_eq!(err.exit_code, None);
        }

        #[test]
        fn missing_executable_is_spawn_failure() {
            let dir = TempDir::new();
            let Outcome::Failed(err) = fetch(&dir.0, &dir.0.join("no-such-bazel")) else {
                panic!("expected Failed");
            };
            assert_eq!(err.exit_code, None);
            assert!(
                err.summary.starts_with("failed to spawn "),
                "{}",
                err.summary
            );
        }
    }
}
