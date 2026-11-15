//! Fast target-list ingestion and on-demand proto details for one target.
//!
//! Triggers: workspace open and manual refresh. Each refresh runs its own
//! scoped query as an owned invocation; Bazel's own lock decides whether
//! queries can run alongside other commands, and the caller's request id
//! decides which reply is current.

use crate::actions;
use crate::query_proto::{self, ParsedQuery};
use crate::runner::{self, Output};
use crate::snapshot::{self, Status, Target, TargetIndex};
use crate::state::Core;
use serde::Serialize;
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestError {
    /// Exit code of the query process, if it ran to an exit.
    pub exit_code: Option<i32>,
    /// Short human-readable summary (tail of stderr, timeout, spawn error).
    pub summary: String,
}

/// The result of one target-list refresh: the parsed index, or the error
/// that prevented it. A partial fetch carries both.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetLoad {
    pub snapshot: Option<TargetIndex>,
    pub error: Option<IngestError>,
}

/// Run the scoped target-list query for `ws` and return its result. Every
/// call is an independent owned invocation; nothing is cached in the core.
pub fn request(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    scope: crate::scope::Scope,
) -> Result<TargetLoad, String> {
    let query = scope.validated()?.rules_query();
    let (ws_path, bazel) = acquire_target(&core, ws)?;
    let id = uuid::Uuid::new_v4().to_string();
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
    Ok(target_load(outcome))
}

fn target_load(outcome: Outcome<Vec<Target>>) -> TargetLoad {
    let (snapshot, error) = match outcome {
        Outcome::Ok(targets) => (Some(snapshot::build_index(targets, Status::Ok)), None),
        Outcome::Partial(targets, error) => (
            Some(snapshot::build_index(targets, Status::Partial)),
            Some(error),
        ),
        Outcome::Failed(error) => (None, Some(error)),
    };
    TargetLoad { snapshot, error }
}

/// Query the selected target in `ws`. The label's format is validated here;
/// membership in the current target list is the caller's check.
#[tracing::instrument(target = "zeal::operations", name = "target.detail", skip_all)]
pub fn target_detail(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    label: &str,
) -> Result<snapshot::TargetDetail, String> {
    let query = crate::scope::target_query(label)?;
    let (ws_path, bazel) = acquire_target(&core, ws)?;
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
) -> Result<TargetOutputs, String> {
    let pattern = crate::scope::cquery_target(label)?;
    let (ws_path, bazel) = acquire_target(&core, ws)?;
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
) -> Result<actions::TargetActions, String> {
    let pattern = crate::scope::cquery_target(label)?;
    let (ws_path, bazel) = acquire_target(&core, ws)?;
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

/// Resolve `ws`'s registered path and the bazel executable. Only that
/// workspace is consulted; nothing here serializes other work.
fn acquire_target(core: &Core, ws: &str) -> Result<(PathBuf, PathBuf), String> {
    let st = core.state.lock().expect("application state lock poisoned");
    let ws_path = st.workspace_path(ws)?;
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
        let load = target_load(fetch_list(&fixture, "success"));
        let snapshot = load.snapshot.expect("success carries an index");
        assert!(load.error.is_none());
        assert!(matches!(snapshot.meta.status, Status::Ok));
        assert_eq!(snapshot.targets.len(), 1);
        assert_eq!(snapshot.targets[0].label, "//app:chosen");
        assert_eq!(snapshot.targets[0].rule_class, "cc_library");
    }

    #[test]
    fn heavy_stderr_preserves_partial_target_list() {
        let fixture = QueryFixture::new();
        let load = target_load(fetch_list(&fixture, "partial"));
        let snapshot = load.snapshot.expect("partial still carries an index");
        assert!(matches!(snapshot.meta.status, Status::Partial));
        assert_eq!(snapshot.targets.len(), 1);
        assert_eq!(snapshot.targets[0].label, "//app:chosen");
        let error = load.error.expect("partial carries its cause");
        assert_eq!(error.exit_code, Some(9));
        assert_eq!(error.summary, "final diagnostic marker");
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

    #[test]
    fn acquire_target_resolves_only_registered_roots_with_bazel() {
        let core = Core::new();
        let ws = PathBuf::from("/ws");
        let other = PathBuf::from("/other");
        {
            let mut st = core.state.lock().unwrap();
            st.register_workspace(ws.clone(), crate::scope::Scope::default());
            st.register_workspace(other.clone(), crate::scope::Scope::default());
            st.bazel = Some(PathBuf::from("/bin/bazel"));
        }
        // An unregistered workspace and a missing bazel are both rejected.
        assert!(acquire_target(&core, "/missing").is_err());
        {
            let mut st = core.state.lock().unwrap();
            st.bazel = None;
        }
        assert!(acquire_target(&core, "/ws").is_err());
        {
            let mut st = core.state.lock().unwrap();
            st.bazel = Some(PathBuf::from("/bin/bazel"));
        }
        let (ws_path, bazel) = acquire_target(&core, "/ws").unwrap();
        assert_eq!(ws_path, ws);
        assert_eq!(bazel, PathBuf::from("/bin/bazel"));
        // Roots resolve independently of each other.
        let (other_path, _) = acquire_target(&core, "/other").unwrap();
        assert_eq!(other_path, other);
    }

    #[test]
    fn heavy_stderr_reports_failed_target_list() {
        let fixture = QueryFixture::new();
        let load = target_load(fetch_list(&fixture, "failed"));
        assert!(load.snapshot.is_none());
        let error = load.error.expect("failure carries an error");
        assert_eq!(error.exit_code, Some(7));
        assert_eq!(error.summary, "final diagnostic marker");
    }
}
