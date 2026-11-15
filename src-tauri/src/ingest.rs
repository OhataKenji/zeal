//! Fast target-list ingestion and on-demand proto details for one target.
//!
//! Triggers: workspace open and manual refresh. Each workspace tracks its own
//! in-flight fetch by request id; a duplicate refresh for the same
//! workspace is rejected, never queued, and Bazel's own lock decides whether
//! queries can run alongside other commands.

use crate::actions;
use crate::query_proto::{self, ParsedQuery};
use crate::runner::{self, Output};
use crate::snapshot::{self, Status, Target, TargetIndex};
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

/// Output paths shown on the target page before the "…" marker.
const MAX_OUTPUT_PATHS: usize = 10;

enum Outcome<T> {
    Ok(T),
    /// exit != 0 but some query data was parsed.
    Partial(T, IngestError),
    Failed(IngestError),
}

/// Start one ingestion for `ws`. A second refresh while one is in flight is
/// rejected rather than queued; it says nothing about other Bazel work.
pub fn request(app: AppHandle, core: Arc<Core>, ws: &str) -> Result<(), String> {
    let (ws_path, bazel, query, id) = {
        let mut st = core.state.lock().expect("application state lock poisoned");
        let ws_path = st.workspace_path(ws)?;
        let bazel = st.bazel.clone().ok_or_else(|| "no bazel".to_string())?;
        let (query, id) = begin_ingest(&mut st, &ws_path)?;
        (ws_path, bazel, query, id)
    };
    core.emit(&app);
    thread::spawn(move || {
        let mut command = command(&ws_path, &bazel, &query, "label_kind");
        let outcome = fetch(
            &core,
            runner::InvocationMeta {
                workspace: &ws_path,
                cmd: "query",
                pattern: &query,
                id: &id,
            },
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
        publish_ingest(&mut st, &ws_path, &id, snapshot, error);
        drop(st);
        core.emit(&app);
    });
    Ok(())
}

/// Claim `ws`'s next ingest request for one refresh. A second refresh
/// while one is in flight is rejected rather than queued; it says nothing
/// about other Bazel work.
fn begin_ingest(st: &mut crate::state::CoreState, ws: &Path) -> Result<(String, String), String> {
    let ws_state = st
        .workspaces
        .get_mut(ws)
        .ok_or_else(|| "workspace is not open".to_string())?;
    if ws_state.ingest_request.is_some() {
        return Err("target list refresh already in flight".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    ws_state.ingest_request = Some(id.clone());
    Ok((ws_state.scope.rules_query(), id))
}

/// Publish a finished fetch only while `id` is still `ws`'s in-flight
/// request: a scope change or a newer refresh supersedes it.
fn publish_ingest(
    st: &mut crate::state::CoreState,
    ws: &Path,
    id: &str,
    snapshot: Option<Arc<TargetIndex>>,
    error: Option<IngestError>,
) {
    if let Some(ws_state) = st.workspaces.get_mut(ws)
        && ws_state.ingest_request.as_deref() == Some(id)
    {
        if let Some(snapshot) = snapshot {
            ws_state.snapshot = Some(snapshot);
        }
        ws_state.last_error = error;
        ws_state.ingest_request = None;
    }
}

/// Query the selected target. The workspace's index guards its label and
/// fetched generation; other workspaces and invocations are unaffected.
#[tracing::instrument(target = "zeal::operations", name = "target.detail", skip_all)]
pub fn target_detail(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    label: &str,
    fetched_at: u64,
) -> Result<snapshot::TargetDetail, String> {
    let query = crate::scope::target_query(label)?;
    let (ws_path, bazel) = acquire_target(&core, ws, label, fetched_at)?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut command = command(&ws_path, &bazel, &query, "proto");
    let (query, status) = match fetch(
        &core,
        runner::InvocationMeta {
            workspace: &ws_path,
            cmd: "query",
            pattern: &query,
            id: &id,
        },
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

/// Output paths kept for the target page; larger results set `truncated`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetOutputs {
    pub paths: Vec<String>,
    pub truncated: bool,
}

/// The target's output files as `bazel cquery --output=files` prints them:
/// configuration-resolved `bazel-out/...` paths, not declared output labels.
#[tracing::instrument(target = "zeal::operations", name = "target.outputs", skip_all)]
pub fn target_outputs(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    label: &str,
    fetched_at: u64,
) -> Result<TargetOutputs, String> {
    let pattern = crate::scope::cquery_target(label)?;
    let (ws_path, bazel) = acquire_target(&core, ws, label, fetched_at)?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut command = Command::new(&bazel);
    command
        .arg("--noblock_for_lock")
        .arg("cquery")
        .arg(&pattern)
        .arg("--output=files")
        .current_dir(&ws_path);
    match fetch(
        &core,
        runner::InvocationMeta {
            workspace: &ws_path,
            cmd: "cquery",
            pattern: &pattern,
            id: &id,
        },
        &mut command,
        || core.emit(&app),
        parse_output_paths,
        |outputs: &TargetOutputs| !outputs.paths.is_empty(),
    ) {
        Outcome::Ok(outputs) | Outcome::Partial(outputs, _) => Ok(outputs),
        Outcome::Failed(error) => Err(error.summary),
    }
}

/// `--output=files` prints one workspace-relative output path per line. Only
/// the first [`MAX_OUTPUT_PATHS`] are kept so filegroup-scale results stay
/// cheap; the rest are reported through `truncated`.
fn parse_output_paths(input: impl BufRead) -> Result<TargetOutputs, String> {
    let mut paths = Vec::new();
    let mut truncated = false;
    for line in input.lines() {
        let line = line.map_err(|_| "could not read cquery output".to_string())?;
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        if paths.len() < MAX_OUTPUT_PATHS {
            paths.push(path.to_string());
        } else {
            truncated = true;
        }
    }
    Ok(TargetOutputs { paths, truncated })
}

#[tracing::instrument(target = "zeal::operations", name = "target.actions", skip_all)]
pub fn target_actions(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    label: &str,
    fetched_at: u64,
) -> Result<actions::TargetActions, String> {
    let pattern = crate::scope::cquery_target(label)?;
    let (ws_path, bazel) = acquire_target(&core, ws, label, fetched_at)?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut command = Command::new(&bazel);
    command
        .arg("--noblock_for_lock")
        .arg("aquery")
        .arg(format!("\"{pattern}\""))
        .args([
            "--output=jsonproto",
            "--include_commandline",
            "--include_artifacts",
        ])
        .current_dir(&ws_path);
    match fetch(
        &core,
        runner::InvocationMeta {
            workspace: &ws_path,
            cmd: "aquery",
            pattern: &pattern,
            id: &id,
        },
        &mut command,
        || core.emit(&app),
        actions::parse,
        |actions: &actions::TargetActions| !actions.actions.is_empty(),
    ) {
        Outcome::Ok(actions) | Outcome::Partial(actions, _) => Ok(actions),
        Outcome::Failed(error) => Err(error.summary),
    }
}

/// Resolve `ws`'s current snapshot for `label` at `fetched_at`. Only that
/// workspace's index is consulted; nothing here serializes other work.
fn acquire_target(
    core: &Core,
    ws: &str,
    label: &str,
    fetched_at: u64,
) -> Result<(PathBuf, PathBuf), String> {
    let st = core.state.lock().expect("application state lock poisoned");
    let ws_path = st.workspace_path(ws)?;
    let index = st
        .workspaces
        .get(&ws_path)
        .and_then(|state| state.snapshot.as_ref())
        .ok_or("target list is not loaded")?;
    if index.meta.fetched_at != fetched_at {
        return Err("target list changed; select the target again".to_string());
    }
    if !index.targets.iter().any(|target| target.label == label) {
        return Err("target is not in the current Scope".to_string());
    }
    let bazel = st.bazel.clone().ok_or_else(|| "no bazel".to_string())?;
    Ok((ws_path, bazel))
}

fn command(ws: &Path, bazel: &Path, query: &str, output: &str) -> Command {
    let mut command = Command::new(bazel);
    command
        .arg("--noblock_for_lock")
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
    meta: runner::InvocationMeta<'_>,
    command: &mut Command,
    emit: impl Fn(),
    parse: impl FnOnce(BufReader<ChildStdout>) -> Result<T, String> + Send + 'static,
    has_data: impl Fn(&T) -> bool,
) -> Outcome<T> {
    let id = meta.id.to_string();
    let bazel = command.get_program().to_string_lossy().into_owned();
    let spawned = {
        let mut st = core.state.lock().expect("application state lock poisoned");
        runner::spawn(&mut st, command, meta, Output::Pipes)
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
    let _slot = runner::InvocationGuard {
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
    let exit = match runner::wait_query(core, &id, &mut child) {
        Ok(exit) => exit,
        Err(summary) => {
            return Outcome::Failed(IngestError {
                exit_code: None,
                summary,
            });
        }
    };
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
case "$3" in
  success) code=0 ;;
  partial) code=9 ;;
  failed) code=7 ;;
  malformed-success) code=0 ;;
  malformed-partial) code=9 ;;
  *) exit 99 ;;
esac
if [ "$3" != failed ]; then
  case "$4" in
    --output=label_kind) printf 'cc_library rule //app:chosen\n' ;;
    --output=proto)
      case "$3" in
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
        let core = Core::new();
        let id = uuid::Uuid::new_v4().to_string();
        let mut command = command(&fixture.workspace, &fixture.bazel, scenario, "label_kind");
        fetch(
            &core,
            runner::InvocationMeta {
                workspace: &fixture.workspace,
                cmd: "query",
                pattern: scenario,
                id: &id,
            },
            &mut command,
            || (),
            snapshot::parse_label_kind,
            |targets: &Vec<Target>| !targets.is_empty(),
        )
    }

    fn fetch_detail(fixture: &QueryFixture, scenario: &str) -> Outcome<ParsedQuery> {
        let core = Core::new();
        let id = uuid::Uuid::new_v4().to_string();
        let mut command = command(&fixture.workspace, &fixture.bazel, scenario, "proto");
        fetch(
            &core,
            runner::InvocationMeta {
                workspace: &fixture.workspace,
                cmd: "query",
                pattern: scenario,
                id: &id,
            },
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
    fn output_paths_skip_blank_lines_and_cap_the_list() {
        let mut input = String::new();
        for i in 0..MAX_OUTPUT_PATHS + 3 {
            input.push_str(&format!("bazel-out/k8/bin/app/out{i}\n"));
        }
        input.push_str("\n  bazel-out/k8/bin/app/last \n");
        let outputs = parse_output_paths(std::io::Cursor::new(input)).unwrap();
        assert_eq!(outputs.paths.len(), MAX_OUTPUT_PATHS);
        assert_eq!(outputs.paths[0], "bazel-out/k8/bin/app/out0");
        assert!(outputs.truncated);

        let outputs = parse_output_paths(std::io::Cursor::new(
            "bazel-out/k8/bin/app/server\n\nbazel-out/k8/bin/app/lib.a\n",
        ))
        .unwrap();
        assert_eq!(outputs.paths.len(), 2);
        assert!(!outputs.truncated);
    }

    fn index_with(fetched_at: u64, labels: &[&str]) -> Arc<TargetIndex> {
        Arc::new(TargetIndex {
            meta: Meta {
                fetched_at,
                status: Status::Ok,
            },
            targets: labels
                .iter()
                .map(|label| Target {
                    label: (*label).into(),
                    pkg: "app".into(),
                    name: label.rsplit(':').next().unwrap().into(),
                    rule_class: "cc_library".into(),
                    location: None,
                })
                .collect(),
        })
    }

    fn workspace_with_snapshot(core: &Core, ws: &Path, fetched_at: u64) {
        let mut st = core.state.lock().unwrap();
        st.register_workspace(ws.to_path_buf(), crate::scope::Scope::default());
        st.workspaces.get_mut(ws).unwrap().snapshot =
            Some(index_with(fetched_at, &["//app:chosen"]));
    }

    #[test]
    fn detail_requires_the_workspaces_current_index_label_and_generation() {
        let core = Core::new();
        let ws = PathBuf::from("/ws");
        {
            let mut st = core.state.lock().unwrap();
            st.bazel = Some(PathBuf::from("/bin/bazel"));
        }
        workspace_with_snapshot(&core, &ws, 7);
        // Another workspace has no index: its labels are never served.
        let other = PathBuf::from("/other");
        {
            let mut st = core.state.lock().unwrap();
            st.register_workspace(other.clone(), crate::scope::Scope::default());
        }
        assert!(acquire_target(&core, "/other", "//app:chosen", 7).is_err());
        assert!(acquire_target(&core, "/ws", "//app:chosen", 6).is_err());
        assert!(acquire_target(&core, "/ws", "//app:unknown", 7).is_err());
        let (ws_path, bazel) = acquire_target(&core, "/ws", "//app:chosen", 7).unwrap();
        assert_eq!(ws_path, ws);
        assert_eq!(bazel, PathBuf::from("/bin/bazel"));
        // Detail reads never serialize each other: the same target can be
        // acquired again while another query is in flight.
        assert!(acquire_target(&core, "/ws", "//app:chosen", 7).is_ok());
    }

    fn assert_ingest_pending(st: &crate::state::CoreState, ws: &Path, id: &str) {
        let state = &st.workspaces[ws];
        assert!(state.snapshot.is_none());
        assert_eq!(state.ingest_request.as_deref(), Some(id));
        assert!(state.last_error.is_none());
    }

    fn assert_snapshot_published(st: &crate::state::CoreState, ws: &Path, fetched_at: u64) {
        let state = &st.workspaces[ws];
        assert_eq!(state.snapshot.as_ref().unwrap().meta.fetched_at, fetched_at);
        assert!(state.ingest_request.is_none());
    }

    #[test]
    fn ingest_requests_dedupe_and_only_the_matching_request_publishes() {
        let core = Core::new();
        let ws = PathBuf::from("/ws");
        let mut st = core.state.lock().unwrap();
        st.register_workspace(ws.clone(), crate::scope::Scope::default());
        let (query, id) = begin_ingest(&mut st, &ws).unwrap();
        assert!(!query.is_empty());
        uuid::Uuid::parse_str(&id).unwrap();
        // A second refresh while one is in flight is rejected, not queued.
        assert!(begin_ingest(&mut st, &ws).is_err());
        assert_ingest_pending(&st, &ws, &id);
        // A foreign id can publish neither a result nor an error.
        publish_ingest(
            &mut st,
            &ws,
            "foreign",
            Some(index_with(1, &[])),
            Some(IngestError {
                exit_code: None,
                summary: "stale".to_string(),
            }),
        );
        assert_ingest_pending(&st, &ws, &id);
        // The matching id publishes and clears the request.
        publish_ingest(&mut st, &ws, &id, Some(index_with(9, &[])), None);
        assert_snapshot_published(&st, &ws, 9);
        // A scope change supersedes the id a fetch captured.
        let stale = begin_ingest(&mut st, &ws).unwrap().1;
        st.register_workspace(
            ws.clone(),
            crate::scope::Scope {
                include: vec!["//app/...".to_string()],
            },
        );
        let fresh = begin_ingest(&mut st, &ws).unwrap().1;
        assert_ne!(fresh, stale);
        publish_ingest(
            &mut st,
            &ws,
            &stale,
            Some(index_with(3, &[])),
            Some(IngestError {
                exit_code: None,
                summary: "stale".to_string(),
            }),
        );
        assert_ingest_pending(&st, &ws, &fresh);
        publish_ingest(
            &mut st,
            &ws,
            &fresh,
            Some(index_with(4, &[])),
            Some(IngestError {
                exit_code: None,
                summary: "failed".to_string(),
            }),
        );
        assert_snapshot_published(&st, &ws, 4);
        assert!(st.workspaces[&ws].last_error.is_some());
        // Scoping away and back cannot restore a cleared request id.
        let gone = begin_ingest(&mut st, &ws).unwrap().1;
        st.register_workspace(ws.clone(), crate::scope::Scope::default());
        st.register_workspace(
            ws.clone(),
            crate::scope::Scope {
                include: vec!["//app/...".to_string()],
            },
        );
        let again = begin_ingest(&mut st, &ws).unwrap().1;
        assert_ne!(again, gone);
        publish_ingest(
            &mut st,
            &ws,
            &gone,
            Some(index_with(5, &[])),
            Some(IngestError {
                exit_code: None,
                summary: "stale".to_string(),
            }),
        );
        assert_ingest_pending(&st, &ws, &again);
    }
}
