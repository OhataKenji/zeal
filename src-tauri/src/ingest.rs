//! Fast target-list ingestion and on-demand proto details for one target.
//!
//! Triggers: workspace open and manual refresh. Busy requests are rejected;
//! no pending query or post-invocation refresh is kept.

use crate::query_proto::{self, ParsedQuery};
use crate::runner::{self, Output};
use crate::snapshot::{self, Status, Target};
use crate::state::{Core, IngestError};
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::Path;
use std::path::PathBuf;
use std::process::{ChildStdout, Command};
use std::sync::Arc;
use std::thread;
use tauri::AppHandle;

/// Maximum retained stderr diagnostic payload. Read storage, allocation
/// capacity, UTF-8 conversion, and total process memory are separate.
const STDERR_TAIL_BYTES: usize = 8 * 1024;

enum Outcome<T> {
    Ok(T),
    /// exit != 0 but some query data was parsed.
    Partial(T, IngestError),
    Failed(IngestError),
}

/// Start one ingestion. Busy requests are rejected rather than queued.
pub fn request(app: AppHandle, core: Arc<Core>) -> Result<(), String> {
    let (ws, bazel, query) = {
        let mut st = core.state.lock().expect("application state lock poisoned");
        if st.bazel_busy() {
            return Err("bazel is busy; refresh after it finishes".to_string());
        }
        let (Some(ws), Some(bazel)) = (st.workspace.clone(), st.bazel.clone()) else {
            return Ok(());
        };
        let query = st.scope.rules_query();
        st.ingest_running = true;
        (ws, bazel, query)
    };
    core.emit(&app);
    thread::spawn(move || {
        let mut command = command(&ws, &bazel, &query, "label_kind");
        let outcome = fetch(
            &core,
            &mut command,
            || core.emit(&app),
            snapshot::parse_label_kind,
            |targets: &Vec<Target>| !targets.is_empty(),
        );
        let (snapshot, error) = match outcome {
            Outcome::Ok(targets) => (
                Some(Arc::new(snapshot::build_index(targets, Status::Ok))),
                None,
            ),
            Outcome::Partial(targets, error) => (
                Some(Arc::new(snapshot::build_index(targets, Status::Partial))),
                Some(error),
            ),
            Outcome::Failed(error) => (None, Some(error)),
        };
        let mut st = core.state.lock().expect("application state lock poisoned");
        if let Some(snapshot) = snapshot {
            st.snapshot = Some(snapshot);
        }
        st.last_error = error;
        st.ingest_running = false;
        drop(st);
        core.emit(&app);
    });
    Ok(())
}

/// Query the selected target. The index guards its label and generation.
#[tracing::instrument(target = "zeal::operations", name = "target.detail", skip_all)]
pub fn target_detail(
    app: AppHandle,
    core: Arc<Core>,
    label: &str,
    fetched_at: u64,
) -> Result<snapshot::TargetDetail, String> {
    let query = crate::scope::target_query(label)?;
    let (ws, bazel) = acquire_target(&core, label, fetched_at)?;
    let _slot = DetailSlot {
        app: app.clone(),
        core: core.clone(),
    };
    core.emit(&app);
    let mut command = command(&ws, &bazel, &query, "proto");
    let (query, status) = match fetch(
        &core,
        &mut command,
        || core.emit(&app),
        query_proto::parse_query,
        |query: &ParsedQuery| !query.rules.is_empty(),
    ) {
        Outcome::Ok(query) => (query, Status::Ok),
        Outcome::Partial(query, _) => (query, Status::Partial),
        Outcome::Failed(error) => return Err(error.summary),
    };
    snapshot::select_detail(snapshot::build(&query, status), label)
        .ok_or_else(|| "target not found in the current Scope".to_string())
}

/// The target's output files as `bazel cquery --output=files` prints them:
/// configuration-resolved `bazel-out/...` paths, not declared output labels.
#[tracing::instrument(target = "zeal::operations", name = "target.outputs", skip_all)]
pub fn target_outputs(
    app: AppHandle,
    core: Arc<Core>,
    label: &str,
    fetched_at: u64,
) -> Result<Vec<String>, String> {
    let pattern = crate::scope::cquery_target(label)?;
    let (ws, bazel) = acquire_target(&core, label, fetched_at)?;
    let _slot = DetailSlot {
        app: app.clone(),
        core: core.clone(),
    };
    core.emit(&app);
    let mut command = Command::new(&bazel);
    command
        .arg("cquery")
        .arg(&pattern)
        .arg("--output=files")
        .current_dir(&ws);
    match fetch(
        &core,
        &mut command,
        || core.emit(&app),
        parse_output_paths,
        |paths: &Vec<String>| !paths.is_empty(),
    ) {
        Outcome::Ok(paths) | Outcome::Partial(paths, _) => Ok(paths),
        Outcome::Failed(error) => Err(error.summary),
    }
}

/// `--output=files` prints one workspace-relative output path per line.
fn parse_output_paths(input: impl BufRead) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for line in input.lines() {
        let line = line.map_err(|_| "could not read cquery output".to_string())?;
        let path = line.trim();
        if !path.is_empty() && !paths.iter().any(|seen| seen == path) {
            paths.push(path.to_string());
        }
    }
    Ok(paths)
}

fn acquire_target(core: &Core, label: &str, fetched_at: u64) -> Result<(PathBuf, PathBuf), String> {
    let mut st = core.state.lock().expect("application state lock poisoned");
    let index = st.snapshot.as_ref().ok_or("target list is not loaded")?;
    if index.meta.fetched_at != fetched_at {
        return Err("target list changed; select the target again".to_string());
    }
    if !index.targets.iter().any(|target| target.label == label) {
        return Err("target is not in the current Scope".to_string());
    }
    if st.bazel_busy() {
        return Err("bazel is busy; load target details after it finishes".to_string());
    }
    let (Some(ws), Some(bazel)) = (st.workspace.clone(), st.bazel.clone()) else {
        return Err("no workspace or bazel".to_string());
    };
    st.detail_query_running = true;
    Ok((ws, bazel))
}

struct DetailSlot {
    app: AppHandle,
    core: Arc<Core>,
}

impl Drop for DetailSlot {
    fn drop(&mut self) {
        let mut st = self
            .core
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        st.detail_query_running = false;
        drop(st);
        if !thread::panicking() {
            self.core.emit(&self.app);
        }
    }
}

fn command(ws: &Path, bazel: &Path, query: &str, output: &str) -> Command {
    let mut command = Command::new(bazel);
    command
        .arg("query")
        .arg(query)
        .arg(format!("--output={output}"))
        .args(["--noimplicit_deps", "--keep_going"])
        .current_dir(ws);
    command
}

/// Run one scoped query and parse stdout.
#[tracing::instrument(target = "zeal::operations", name = "bazel.query", skip_all)]
fn fetch<T: Send + 'static>(
    core: &Core,
    command: &mut Command,
    emit: impl Fn(),
    parse: impl FnOnce(BufReader<ChildStdout>) -> Result<T, String> + Send + 'static,
    has_data: impl Fn(&T) -> bool,
) -> Outcome<T> {
    let id = uuid::Uuid::new_v4().to_string();
    let bazel = command.get_program().to_string_lossy().into_owned();
    let spawned = {
        let mut st = core.state.lock().expect("application state lock poisoned");
        runner::spawn(&mut st, command, &id, Output::Pipes)
    };
    let (mut child, _) = match spawned {
        Ok(child) => child,
        Err(error) => {
            return Outcome::Failed(IngestError {
                exit_code: None,
                summary: format!("failed to spawn {bazel}: {error}"),
            });
        }
    };
    let _slot = runner::BazelSlot {
        core,
        id: &id,
        emit: &emit,
    };
    emit();
    // Read stdout and drain stderr concurrently so the child cannot block.
    let stdout = child.stdout.take().expect("stdout piped");
    let parse_handle = thread::spawn(move || parse(BufReader::new(stdout)));
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stderr_handle = thread::spawn(move || capture_stderr_tail(&mut stderr));
    let waited = runner::wait(core, &id, || {
        child
            .try_wait()
            .map(|status| status.map(|status| status.code().unwrap_or(-1)))
    });
    let exit = match waited {
        Ok(exit) => exit,
        Err(error) => {
            return Outcome::Failed(IngestError {
                exit_code: None,
                summary: format!("failed to wait on query: {error}"),
            });
        }
    };
    let stopping = core
        .state
        .lock()
        .expect("application state lock poisoned")
        .active_invocation
        .as_ref()
        .is_some_and(|invocation| invocation.id.as_str() == id && invocation.stopping);
    if stopping {
        return Outcome::Failed(IngestError {
            exit_code: None,
            summary: "Query stopped".to_string(),
        });
    }
    let parsed = parse_handle
        .join()
        .unwrap_or_else(|_| Err("query output parser failed".to_string()));
    let stderr_bytes = stderr_handle.join().unwrap_or_default();
    let stderr_tail = String::from_utf8_lossy(&stderr_bytes);
    let code = (exit >= 0).then_some(exit);
    let data = match parsed {
        Ok(data) => data,
        Err(summary) => {
            return Outcome::Failed(IngestError {
                exit_code: code,
                summary,
            });
        }
    };
    match code {
        Some(0) => Outcome::Ok(data),
        _ if has_data(&data) => Outcome::Partial(
            data,
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

/// Drain stderr while retaining at most its final diagnostic bytes.
fn capture_stderr_tail(stderr: &mut impl Read) -> Vec<u8> {
    let mut tail = Vec::with_capacity(STDERR_TAIL_BYTES);
    let mut chunk = [0u8; 8 * 1024];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let kept = n.min(STDERR_TAIL_BYTES);
                let dropped = (tail.len() + kept).saturating_sub(STDERR_TAIL_BYTES);
                tail.drain(..dropped);
                tail.extend_from_slice(&chunk[n - kept..n]);
                debug_assert!(tail.len() <= STDERR_TAIL_BYTES);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    tail
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
    use crate::protocol::query as query_schema;
    use crate::snapshot::{Meta, TargetIndex};
    use prost::Message;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn stderr_tail_is_bounded_independently_of_total_output() {
        let mut output = vec![b'x'; 300_000];
        output.extend_from_slice(b"\nfinal diagnostic marker\n");
        let mut reader = output.as_slice();
        let tail = capture_stderr_tail(&mut reader);
        assert!(reader.is_empty(), "stderr was not fully drained");
        assert_eq!(tail.len(), STDERR_TAIL_BYTES);
        assert_eq!(tail, output[output.len() - STDERR_TAIL_BYTES..]);
    }

    #[test]
    fn stderr_tail_handles_split_utf8_and_keeps_a_valid_summary() {
        let marker = b"\ncontext one\ncontext two\nfinal diagnostic marker\n";
        let mut output = "界".as_bytes().to_vec();
        output.extend(vec![b'x'; STDERR_TAIL_BYTES - marker.len() - 1]);
        output.extend_from_slice(marker);
        let mut reader = (&output[..2]).chain(&output[2..]);
        let tail = capture_stderr_tail(&mut reader);
        assert_eq!(tail.len(), STDERR_TAIL_BYTES);
        assert_eq!(tail[0], "界".as_bytes()[2]);
        let text = String::from_utf8_lossy(&tail);
        assert!(text.starts_with('\u{fffd}'));
        assert_eq!(
            summarize(&text),
            "context one | context two | final diagnostic marker"
        );
    }

    struct QueryFixture {
        workspace: PathBuf,
        bazel: PathBuf,
    }

    impl QueryFixture {
        fn new() -> Self {
            let workspace =
                std::env::temp_dir().join(format!("zeal-ingest-stderr-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&workspace).unwrap();
            let result = query_schema::QueryResult {
                target: vec![query_schema::Target {
                    r#type: query_schema::target::Discriminator::Rule as i32,
                    rule: Some(query_schema::Rule {
                        name: "//app:chosen".to_string(),
                        rule_class: "cc_library".to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
            };
            fs::write(workspace.join("query.pb"), result.encode_to_vec()).unwrap();
            fs::write(workspace.join("bad.pb"), [0x0a, 0x80]).unwrap();
            let bazel = workspace.join("bazel");
            fs::write(
                &bazel,
                r#"#!/bin/sh
i=0
while [ "$i" -lt 40 ]; do
  printf '%8192s' '' >&2
  i=$((i + 1))
done
printf 'final diagnostic marker\n' >&2
case "$2" in
  success) code=0 ;;
  partial) code=9 ;;
  failed) code=7 ;;
  malformed-success) code=0 ;;
  malformed-partial) code=9 ;;
  *) exit 99 ;;
esac
if [ "$2" != failed ]; then
  case "$3" in
    --output=label_kind) printf 'cc_library rule //app:chosen\n' ;;
    --output=proto)
      case "$2" in
        malformed-*) cat bad.pb ;;
        *) cat query.pb ;;
      esac
      ;;
    *) exit 99 ;;
  esac
fi
exit "$code"
"#,
            )
            .unwrap();
            fs::set_permissions(&bazel, fs::Permissions::from_mode(0o700)).unwrap();
            Self { workspace, bazel }
        }
    }

    impl Drop for QueryFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.workspace);
        }
    }

    fn fetch_list(fixture: &QueryFixture, scenario: &str) -> Outcome<Vec<Target>> {
        let mut command = command(&fixture.workspace, &fixture.bazel, scenario, "label_kind");
        fetch(
            &Core::new(),
            &mut command,
            || (),
            snapshot::parse_label_kind,
            |targets: &Vec<Target>| !targets.is_empty(),
        )
    }

    fn fetch_detail(fixture: &QueryFixture, scenario: &str) -> Outcome<ParsedQuery> {
        let mut command = command(&fixture.workspace, &fixture.bazel, scenario, "proto");
        fetch(
            &Core::new(),
            &mut command,
            || (),
            query_proto::parse_query,
            |query: &ParsedQuery| !query.rules.is_empty(),
        )
    }

    #[test]
    fn heavy_stderr_does_not_block_target_list_success() {
        let fixture = QueryFixture::new();
        assert!(
            matches!(fetch_list(&fixture, "success"), Outcome::Ok(targets) if targets.len() == 1)
        );
    }

    #[test]
    fn heavy_stderr_preserves_partial_target_list() {
        let fixture = QueryFixture::new();
        assert!(
            matches!(fetch_list(&fixture, "partial"), Outcome::Partial(targets, error)
            if targets.len() == 1 && error.exit_code == Some(9) && error.summary == "final diagnostic marker")
        );
    }

    #[test]
    fn heavy_stderr_allows_successful_selected_target_detail() {
        let fixture = QueryFixture::new();
        assert!(
            matches!(fetch_detail(&fixture, "success"), Outcome::Ok(query)
            if query.rules.len() == 1 && query.rules[0].label == "//app:chosen")
        );
    }

    #[test]
    fn heavy_stderr_preserves_partial_selected_target_detail() {
        let fixture = QueryFixture::new();
        assert!(
            matches!(fetch_detail(&fixture, "partial"), Outcome::Partial(query, error)
            if query.rules.len() == 1 && error.exit_code == Some(9) && error.summary == "final diagnostic marker")
        );
    }

    #[test]
    fn malformed_proto_fails_despite_zero_exit() {
        let fixture = QueryFixture::new();
        assert!(
            matches!(fetch_detail(&fixture, "malformed-success"), Outcome::Failed(error)
            if error.exit_code == Some(0) && error.summary.contains("protobuf"))
        );
    }

    #[test]
    fn malformed_proto_fails_partial_output() {
        let fixture = QueryFixture::new();
        assert!(
            matches!(fetch_detail(&fixture, "malformed-partial"), Outcome::Failed(error)
            if error.exit_code == Some(9) && error.summary.contains("protobuf"))
        );
    }

    #[test]
    fn heavy_stderr_reports_failed_selected_target_detail() {
        let fixture = QueryFixture::new();
        assert!(
            matches!(fetch_detail(&fixture, "failed"), Outcome::Failed(error)
            if error.exit_code == Some(7) && error.summary == "final diagnostic marker")
        );
    }

    #[test]
    fn output_paths_skip_blank_lines_and_dedupe() {
        let paths = parse_output_paths(std::io::Cursor::new(
            "bazel-out/k8/bin/app/server\n\n  bazel-out/k8/bin/app/lib.a \nbazel-out/k8/bin/app/server\n",
        ))
        .unwrap();
        assert_eq!(
            paths,
            [
                "bazel-out/k8/bin/app/server".to_string(),
                "bazel-out/k8/bin/app/lib.a".to_string()
            ]
        );
    }

    #[test]
    fn detail_requires_the_current_index_label_and_free_bazel_slot() {
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.workspace = Some(PathBuf::from("/ws"));
            st.bazel = Some(PathBuf::from("/bin/bazel"));
            st.snapshot = Some(Arc::new(TargetIndex {
                meta: Meta {
                    fetched_at: 7,
                    status: Status::Ok,
                },
                targets: vec![Target {
                    label: "//app:chosen".into(),
                    pkg: "app".into(),
                    name: "chosen".into(),
                    rule_class: "cc_library".into(),
                    location: None,
                }],
            }));
        }
        assert!(acquire_target(&core, "//app:chosen", 6).is_err());
        assert!(acquire_target(&core, "//app:unknown", 7).is_err());
        let (ws, bazel) = acquire_target(&core, "//app:chosen", 7).unwrap();
        assert_eq!(ws, PathBuf::from("/ws"));
        assert_eq!(bazel, PathBuf::from("/bin/bazel"));
        assert!(core.state.lock().unwrap().detail_query_running);
        assert!(acquire_target(&core, "//app:chosen", 7).is_err());
    }
}
