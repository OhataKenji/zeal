//! Fast target-list ingestion and on-demand proto details for one target.
//!
//! Triggers: workspace open and manual refresh. Each refresh runs its own
//! scoped query as an owned invocation; Bazel's own lock decides whether
//! queries can run alongside other commands, and the caller's request id
//! decides which reply is current.

use crate::actions;
use crate::configured;
use crate::query_proto::{self, ParsedQuery};
use crate::runner;
use crate::snapshot::{self, Status, Target, TargetIndex};
use crate::state::Core;
use serde::{Deserialize, Serialize};
use std::io::{BufReader, ErrorKind, Read};
use std::process::{Child, ChildStdout};
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
    let outcome = fetch(
        &core,
        runner::spawn_query(
            &core,
            ws,
            "query",
            &query,
            &query_args(&query, "label_kind"),
        ),
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
#[tracing::instrument(target = "zelith::operations", name = "target.detail", skip_all)]
pub fn target_detail(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    label: &str,
) -> Result<snapshot::TargetDetail, String> {
    let query = crate::scope::target_query(label)?;
    let (query, status) = match fetch(
        &core,
        runner::spawn_query(&core, ws, "query", &query, &query_args(&query, "proto")),
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

/// Which single inspector read a `read_target` call performs. Each variant
/// is one Bazel process; the frontend stages the Configured tab's three
/// reads so a tab, flag, or selection change can stop between them.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetRead {
    /// `cquery --output=jsonproto` `config(label, target)` summary.
    Configuration,
    /// `cquery --output=transitions` `config(label, target)` dependency rows.
    Transitions,
    /// `cquery --output=files` `config(label, target)` outputs.
    Outputs,
    /// `aquery` action list for the quoted label.
    Actions,
}

impl TargetRead {
    /// Each cquery asks about the selected target's configuration; actions
    /// quotes the label for aquery.
    fn command(
        self,
        label: &str,
        flags: &str,
    ) -> Result<(Vec<String>, &'static str, String), String> {
        let (kind, managed): (crate::query::QueryKind, &[&str]) = match self {
            Self::Actions => (
                crate::query::QueryKind::Aquery,
                &[
                    "--output=jsonproto",
                    "--include_commandline",
                    "--include_artifacts",
                ],
            ),
            Self::Configuration => (
                crate::query::QueryKind::Cquery,
                &["--output=jsonproto", "--proto:include_configurations"],
            ),
            Self::Transitions => (
                crate::query::QueryKind::Cquery,
                &["--output=transitions", "--transitions=lite"],
            ),
            Self::Outputs => (crate::query::QueryKind::Cquery, &["--output=files"]),
        };
        let label = crate::scope::cquery_target(label)?;
        let expr = if matches!(self, Self::Actions) {
            format!("\"{label}\"")
        } else {
            format!("config(\"{label}\", target)")
        };
        let mut args = crate::query::build_flags(kind, flags)?;
        args.extend(managed.iter().map(|s| (*s).to_owned()));
        args.extend(["--".into(), expr.clone()]);
        Ok((args, kind.as_str(), expr))
    }

    /// Parse this step's stdout into its bare payload.
    fn parse(self, input: BufReader<ChildStdout>) -> Result<TargetReadData, String> {
        match self {
            Self::Configuration => {
                configured::parse_configuration(input).map(TargetReadData::Configuration)
            }
            Self::Transitions => {
                configured::parse_transitions(input).map(TargetReadData::Transitions)
            }
            Self::Outputs => configured::parse_output_paths(input).map(TargetReadData::Outputs),
            Self::Actions => actions::parse(input).map(TargetReadData::Actions),
        }
    }
}

/// One inspector read's untagged result: the bare payload of the requested
/// step (`{"mnemonic":…}`, `{"deps":[…]}`, `{"paths":[…]}`, or
/// `{"actions":[…]}`).
#[derive(Serialize)]
#[serde(untagged)]
pub enum TargetReadData {
    Configuration(Option<configured::ConfigSummary>),
    Transitions(configured::TargetTransitions),
    Outputs(configured::TargetOutputs),
    Actions(actions::TargetActions),
}

/// One inspector read for the selected target under the applied build
/// flags: a single `cquery`/`aquery` process per call. Definition never
/// reaches this command.
#[tracing::instrument(target = "zelith::operations", name = "target.read", skip_all)]
pub fn read_target(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    label: &str,
    flags: &str,
    step: TargetRead,
) -> Result<TargetReadData, String> {
    let (args, cmd, expr) = step.command(label, flags)?;
    match fetch(
        &core,
        runner::spawn_query(&core, ws, cmd, &expr, &args),
        || core.emit(&app),
        move |input| step.parse(input),
        |_| false,
    ) {
        Outcome::Ok(data) | Outcome::Partial(data, _) => Ok(data),
        Outcome::Failed(error) => Err(error.summary),
    }
}

fn query_args(query: &str, output: &str) -> Vec<String> {
    vec![
        query.into(),
        format!("--output={output}"),
        "--noimplicit_deps".into(),
        "--keep_going".into(),
    ]
}

/// Run one scoped query and parse stdout.
#[tracing::instrument(target = "zelith::operations", name = "bazel.query", skip_all)]
fn fetch<T: Send + 'static>(
    core: &Core,
    spawned: Result<(Child, String), String>,
    emit: impl Fn(),
    parse: impl FnOnce(BufReader<ChildStdout>) -> Result<T, String> + Send + 'static,
    has_data: impl Fn(&T) -> bool,
) -> Outcome<T> {
    let (mut child, id) = match spawned {
        Ok(spawned) => spawned,
        Err(summary) => {
            return Outcome::Failed(IngestError {
                exit_code: None,
                summary,
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
        Err(parse_error) => {
            // A nonzero Bazel exit can truncate stdout, so the parser error
            // alone misleads; keep the real stderr diagnostic alongside it.
            let summary = if code != Some(0) && !stderr_tail.trim().is_empty() {
                format!("{} ({})", summarize(&stderr_tail), parse_error)
            } else {
                parse_error
            };
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
    use std::path::PathBuf;

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
        core: Core,
    }

    impl QueryFixture {
        fn new() -> Self {
            let workspace =
                std::env::temp_dir().join(format!("zelith-ingest-stderr-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&workspace).unwrap();
            let workspace = workspace.canonicalize().unwrap();
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
while [ "$#" -gt 0 ] && [ "$1" != query ]; do shift; done
[ "$1" = query ] || exit 99
shift
for arg do
  case "$arg" in
    --output=*) output=$arg ;;
    -*) ;;
    *) scenario=$arg ;;
  esac
done
case "$scenario" in
  success) code=0 ;;
  partial) code=9 ;;
  failed) code=7 ;;
  malformed-success) code=0 ;;
  malformed-partial) code=9 ;;
  *) exit 99 ;;
esac
if [ "$scenario" != failed ]; then
  case "$output" in
    --output=label_kind) printf 'cc_library rule //app:chosen\n' ;;
    --output=proto)
      case "$scenario" in
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
            let core = Core::new();
            let mut st = core.state.lock().unwrap();
            st.workspaces.insert(workspace.clone(), Default::default());
            st.bazel = Some(bazel);
            st.receiver = crate::state::ReceiverState::Listening {
                endpoint: "grpc://127.0.0.1:1".into(),
                warning: None,
            };
            drop(st);
            Self { workspace, core }
        }
    }

    impl Drop for QueryFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.workspace);
        }
    }

    fn fetch_list(fixture: &QueryFixture, scenario: &str) -> Outcome<Vec<Target>> {
        fetch(
            &fixture.core,
            runner::spawn_query(
                &fixture.core,
                fixture.workspace.to_str().unwrap(),
                "query",
                scenario,
                &query_args(scenario, "label_kind"),
            ),
            || (),
            snapshot::parse_label_kind,
            |targets: &Vec<Target>| !targets.is_empty(),
        )
    }

    fn fetch_detail(fixture: &QueryFixture, scenario: &str) -> Outcome<ParsedQuery> {
        fetch(
            &fixture.core,
            runner::spawn_query(
                &fixture.core,
                fixture.workspace.to_str().unwrap(),
                "query",
                scenario,
                &query_args(scenario, "proto"),
            ),
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
        // A nonzero exit with stderr keeps Bazel's diagnostic next to the
        // parser detail instead of surfacing only the misleading parse error.
        assert!(
            matches!(fetch_detail(&fixture, "malformed-partial"), Outcome::Failed(error)
            if error.exit_code == Some(9)
                && error.summary.contains("final diagnostic marker")
                && error.summary.contains("protobuf"))
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
    fn spawn_query_rejects_an_unregistered_workspace_without_a_child() {
        let fixture = QueryFixture::new();
        let spawned = runner::spawn_query(
            &fixture.core,
            "/missing",
            "query",
            "success",
            &query_args("success", "label_kind"),
        );
        assert_eq!(spawned.unwrap_err(), "workspace is not open");
        assert!(fixture.core.state.lock().unwrap().invocations.is_empty());
    }

    #[test]
    fn fetch_rejects_missing_bazel() {
        let fixture = QueryFixture::new();
        fixture.core.state.lock().unwrap().bazel = None;
        assert!(
            matches!(fetch_list(&fixture, "success"), Outcome::Failed(error)
            if error.exit_code.is_none() && error.summary.contains("no bazel"))
        );
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

    #[test]
    fn query_args_request_partial_output_without_implicit_deps() {
        assert_eq!(
            query_args("//...", "label_kind"),
            [
                "//...",
                "--output=label_kind",
                "--noimplicit_deps",
                "--keep_going"
            ]
        );
    }

    #[test]
    fn read_command_scopes_every_cquery_step_to_the_target_configuration() {
        let managed = [
            (
                TargetRead::Configuration,
                &["--output=jsonproto", "--proto:include_configurations"][..],
            ),
            (
                TargetRead::Transitions,
                &["--output=transitions", "--transitions=lite"][..],
            ),
            (TargetRead::Outputs, &["--output=files"][..]),
        ];
        for (step, options) in managed {
            let (args, kind, expr) = step.command("//app:t", "--config=opt").unwrap();
            assert_eq!(kind, "cquery");
            assert_eq!(expr, "config(\"//app:t\", target)");
            let mut expected = vec!["--config=opt"];
            expected.extend(options.iter().copied());
            expected.extend(["--", "config(\"//app:t\", target)"]);
            assert_eq!(args, expected);
        }
    }

    #[test]
    fn read_command_quotes_the_label_for_aquery() {
        let (args, kind, expr) = TargetRead::Actions
            .command("//app:t", "--config=opt")
            .unwrap();
        assert_eq!(kind, "aquery");
        assert_eq!(expr, "\"//app:t\"");
        assert_eq!(
            args,
            [
                "--config=opt",
                "--output=jsonproto",
                "--include_commandline",
                "--include_artifacts",
                "--",
                "\"//app:t\"",
            ]
        );
    }

    #[test]
    fn read_command_rejects_managed_flag_overrides_before_launching() {
        // build_flags runs inside command(), so an --output or managed
        // option override can never reach a phase.
        for step in [
            TargetRead::Configuration,
            TargetRead::Transitions,
            TargetRead::Outputs,
            TargetRead::Actions,
        ] {
            assert!(step.command("//app:t", "--output=files").is_err());
        }
    }

    #[test]
    fn read_data_keeps_its_untagged_wire_shapes() {
        // Each step serializes as its bare payload — no phase tag.
        let summary = configured::ConfigSummary {
            id: 7,
            mnemonic: "k8-opt".to_string(),
            checksum: "abc123".to_string(),
        };
        assert_eq!(
            serde_json::to_value(TargetReadData::Configuration(Some(summary))).unwrap(),
            serde_json::json!({"mnemonic": "k8-opt", "checksum": "abc123"})
        );
        assert_eq!(
            serde_json::to_value(TargetReadData::Configuration(None)).unwrap(),
            serde_json::json!(null)
        );
        assert_eq!(
            serde_json::to_value(TargetReadData::Transitions(configured::TargetTransitions {
                deps: vec![],
                deps_truncated: false,
            }))
            .unwrap(),
            serde_json::json!({"deps": [], "depsTruncated": false})
        );
        assert_eq!(
            serde_json::to_value(TargetReadData::Outputs(configured::TargetOutputs {
                paths: vec![],
                truncated: false,
            }))
            .unwrap(),
            serde_json::json!({"paths": [], "truncated": false})
        );
        assert_eq!(
            serde_json::to_value(TargetReadData::Actions(actions::TargetActions {
                actions: vec![],
                truncated: false,
            }))
            .unwrap(),
            serde_json::json!({"actions": [], "truncated": false})
        );
    }

    #[test]
    fn target_read_deserializes_lowercase_steps() {
        for json in [
            "\"configuration\"",
            "\"transitions\"",
            "\"outputs\"",
            "\"actions\"",
        ] {
            serde_json::from_str::<TargetRead>(json).unwrap();
        }
    }
}
