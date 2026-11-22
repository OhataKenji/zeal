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
use serde::{Deserialize, Serialize};
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

/// Dependency edges kept for the Configured tab; larger results set
/// `depsTruncated`.
const MAX_CONFIGURED_DEPS: usize = 200;

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

/// The selected target's own configuration, resolved by the `config()`
/// cquery expression under the applied build flags.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSummary {
    pub mnemonic: String,
    pub checksum: String,
}

/// One direct dependency edge of the configured target. `transition` is a
/// short badge such as `exec`; `None` for edges that keep the parent's
/// configuration or have none (source files).
#[derive(PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredDep {
    pub label: String,
    pub transition: Option<String>,
}

/// Everything the Configured inspector tab needs for one target: its own
/// configuration summary, `cquery --output=files` outputs, and direct
/// dependency edges with transition badges.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredTarget {
    pub configuration: Option<ConfigSummary>,
    pub outputs: TargetOutputs,
    pub deps: Vec<ConfiguredDep>,
    pub deps_truncated: bool,
}

/// The target's dependency edges, parsed from `cquery
/// --output=transitions --transitions=lite` over `deps(config(...))`.
struct TargetTransitions {
    deps: Vec<ConfiguredDep>,
    deps_truncated: bool,
}

/// Everything the Configured inspector tab needs for one target. Three
/// sequential owned reads share the workspace lock: `config(label, target)`
/// names the target's own configuration under the applied flags, then
/// `deps(config(...), 1)` transitions and `config(...)` files are both
/// scoped to that exact configuration, so retained copies of the same
/// label under other configurations cannot leak in.
#[tracing::instrument(target = "zeal::operations", name = "target.configured", skip_all)]
pub fn configured_target(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    label: &str,
    flags: &str,
) -> Result<ConfiguredTarget, String> {
    let pattern = crate::scope::cquery_target(label)?;
    let flags = crate::query::build_flags(crate::query::QueryKind::Cquery, flags)?;
    let reader = TargetReader::new(&core, &app, ws)?;

    let root = format!("config(\"{pattern}\", target)");
    let configuration = reader.read(
        "cquery",
        &flags,
        &["--output=jsonproto", "--proto:include_configurations"],
        &root,
        parse_configuration,
    )?;

    let checksum = configuration.as_ref().map(|config| config.checksum.clone());
    let transitions = reader.read(
        "cquery",
        &flags,
        &["--output=transitions", "--transitions=lite"],
        &format!("deps({root}, 1)"),
        {
            let label = label.to_string();
            move |input| parse_transitions(input, &label, checksum.as_deref())
        },
    )?;

    let outputs = reader.read(
        "cquery",
        &flags,
        &["--output=files"],
        &root,
        parse_output_paths,
    )?;

    Ok(ConfiguredTarget {
        configuration,
        outputs,
        deps: transitions.deps,
        deps_truncated: transitions.deps_truncated,
    })
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

/// Parse the transitions report for `label` under `checksum` — the full
/// config hash `config()` reported, or `None` for the null configuration.
/// `deps(config(...))` yields exactly one matching header; same-label
/// headers under other retained configurations are ignored.
fn parse_transitions(
    input: impl BufRead,
    label: &str,
    checksum: Option<&str>,
) -> Result<TargetTransitions, String> {
    let mut found = false;
    let mut collecting = false;
    let mut deps = Vec::new();
    let mut deps_truncated = false;
    for line in input.lines() {
        let line = line.map_err(|_| "could not read cquery output".to_string())?;
        if line.starts_with(char::is_whitespace) {
            collect_dep(&line, collecting, &mut deps, &mut deps_truncated);
            continue;
        }
        // Only the first matching header collects; later headers — even a
        // repeated one — end collection for good.
        collecting = !found && header_matches_label(&line, label, checksum);
        found |= collecting;
    }
    if !found {
        return Err(format!("cquery reported no configuration for {label}"));
    }
    Ok(TargetTransitions {
        deps,
        deps_truncated,
    })
}

/// Keep one indented row's dependency while `collecting` — deduplicated and
/// bounded by [`MAX_CONFIGURED_DEPS`]; overflow only sets the flag.
fn collect_dep(
    line: &str,
    collecting: bool,
    deps: &mut Vec<ConfiguredDep>,
    deps_truncated: &mut bool,
) {
    if !collecting {
        return;
    }
    let Some(dep) = parse_transition_row(line) else {
        return;
    };
    if deps.contains(&dep) {
        return;
    }
    if deps.len() < MAX_CONFIGURED_DEPS {
        deps.push(dep);
    } else {
        *deps_truncated = true;
    }
}

/// A column-0 header names `label` under the selected configuration.
fn header_matches_label(line: &str, label: &str, checksum: Option<&str>) -> bool {
    let Some((header_label, id)) = parse_transition_header(line) else {
        return false;
    };
    header_label == label && header_matches(id, checksum)
}

/// A header belongs to the selected configuration when its short checksum
/// prefixes the full hash `config()` reported — or is `null` for targets
/// without a configuration.
fn header_matches(id: &str, checksum: Option<&str>) -> bool {
    match checksum {
        Some(sum) => sum.starts_with(id),
        None => id == "null",
    }
}

/// A column-0 transitions header: `[<rule transition> -> ]<label> (<id>)`,
/// where `<id>` is the configuration checksum or `null`. Labels cannot
/// contain whitespace, so the last ` -> ` separates the optional
/// rule-class transition prefix from the label.
fn parse_transition_header(line: &str) -> Option<(&str, &str)> {
    if line.starts_with(char::is_whitespace) {
        return None;
    }
    let (prefix, rest) = line.rsplit_once(" (")?;
    let id = rest.strip_suffix(')')?;
    let label = prefix.rsplit_once(" -> ").map_or(prefix, |(_, l)| l);
    (!label.is_empty() && !id.is_empty()).then_some((label, id))
}

/// One `attr#label#transition -> <ids>` row under a header. The attribute is
/// the first `#` field and the transition name — which never contains `#` —
/// is the last, so labels containing `#` stay intact.
fn parse_transition_row(line: &str) -> Option<ConfiguredDep> {
    let (head, _) = line.rsplit_once("->")?;
    let (_, rest) = head.trim_end().split_once('#')?;
    let (label, transition) = rest.rsplit_once('#')?;
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    Some(ConfiguredDep {
        label: label.to_string(),
        transition: transition_badge(transition.trim()),
    })
}

/// The badge shown for a dependency edge, or `None` when the edge keeps the
/// parent's configuration (`NoTransition`) or has none (`(null transition)`,
/// e.g. source files). Composite `exec` transitions badge as `exec`; every
/// other name is kept whole for an honest display.
fn transition_badge(name: &str) -> Option<String> {
    match name {
        "(null transition)" | "NoTransition" => None,
        "exec" => Some("exec".to_string()),
        _ if name.starts_with("(exec + ") => Some("exec".to_string()),
        _ => Some(name.to_string()),
    }
}

/// The `configurations` entry `config()` resolves to. Only the fields the
/// inspector shows are declared — Bazel's remaining option payload is
/// ignored rather than materialized.
#[derive(Deserialize)]
struct NamedConfig {
    id: u32,
    mnemonic: String,
    checksum: String,
}

/// One `cquery --output=jsonproto` result's configuration reference.
#[derive(Deserialize)]
struct ResultConfig {
    #[serde(rename = "configurationId", default)]
    id: u32,
}

/// `cquery --output=jsonproto` top level: the results plus the named
/// configurations they reference.
#[derive(Deserialize)]
struct ConfigQuery {
    #[serde(default)]
    results: Vec<ResultConfig>,
    #[serde(default)]
    configurations: Vec<NamedConfig>,
}

/// `config("label", target)` answers with exactly one result; its
/// `configurationId` names the entry in `configurations` — absent or zero
/// for targets in the null configuration (source files).
fn parse_configuration(input: impl BufRead) -> Result<Option<ConfigSummary>, String> {
    let query: ConfigQuery = serde_json::from_reader(input)
        .map_err(|e| format!("could not parse cquery configuration output: {e}"))?;
    if query.results.len() != 1 {
        return Err(format!(
            "cquery config() returned {} results",
            query.results.len()
        ));
    }
    let id = query.results[0].id;
    if id == 0 {
        return Ok(None);
    }
    query
        .configurations
        .into_iter()
        .find(|config| config.id == id)
        .map(|config| {
            Some(ConfigSummary {
                mnemonic: config.mnemonic,
                checksum: config.checksum,
            })
        })
        .ok_or_else(|| format!("cquery config() referenced unknown configuration {id}"))
}

#[tracing::instrument(target = "zeal::operations", name = "target.actions", skip_all)]
pub fn target_actions(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    label: &str,
    flags: &str,
) -> Result<actions::TargetActions, String> {
    let pattern = crate::scope::cquery_target(label)?;
    let flags = crate::query::build_flags(crate::query::QueryKind::Aquery, flags)?;
    let reader = TargetReader::new(&core, &app, ws)?;
    reader.read(
        "aquery",
        &flags,
        &ACTION_MANAGED,
        &format!("\"{pattern}\""),
        actions::parse,
    )
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

/// One IPC's worth of inspector reads against a single workspace: the
/// resolved path and bazel executable plus the app handle for progress
/// events. Every `read` is an owned invocation under the workspace lock.
struct TargetReader<'a> {
    core: &'a Core,
    app: &'a AppHandle,
    ws: PathBuf,
    bazel: PathBuf,
}

impl<'a> TargetReader<'a> {
    fn new(core: &'a Core, app: &'a AppHandle, ws: &str) -> Result<Self, String> {
        let (ws, bazel) = acquire_target(core, ws)?;
        Ok(Self {
            core,
            app,
            ws,
            bazel,
        })
    }

    /// Run one `bazel <cmd>` read and parse stdout. Inspector reads treat any
    /// nonzero exit as an error — a partial parse would mislead.
    fn read<T: Send + 'static>(
        &self,
        cmd: &str,
        flags: &[String],
        managed: &[&str],
        expr: &str,
        parse: impl FnOnce(BufReader<ChildStdout>) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let args = target_args(cmd, flags, managed, expr);
        let mut command = Command::new(&self.bazel);
        command.args(&args).current_dir(&self.ws);
        match fetch(
            self.core,
            runner::InvocationMeta {
                workspace: &self.ws,
                cmd,
                pattern: expr,
                id: &id,
            },
            &mut command,
            || self.core.emit(self.app),
            parse,
            |_| false,
        ) {
            Outcome::Ok(value) | Outcome::Partial(value, _) => Ok(value),
            Outcome::Failed(error) => Err(error.summary),
        }
    }
}

/// `bazel <cmd>` argv shared by the inspector reads: user flags, then the
/// managed options, then `--` and the expression.
fn target_args(cmd: &str, flags: &[String], managed: &[&str], expr: &str) -> Vec<String> {
    let mut args = vec!["--noblock_for_lock".to_string(), cmd.to_string()];
    args.extend(flags.iter().cloned());
    args.extend(managed.iter().map(|arg| arg.to_string()));
    args.push("--".to_string());
    args.push(expr.to_string());
    args
}

/// Managed aquery options for the Actions read: the output shape is fixed,
/// so user flags must not be able to override it.
const ACTION_MANAGED: [&str; 3] = [
    "--output=jsonproto",
    "--include_commandline",
    "--include_artifacts",
];

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
            st.workspaces
                .insert(ws.clone(), crate::scope::Scope::default());
            st.workspaces
                .insert(other.clone(), crate::scope::Scope::default());
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

    const TRANSITIONS_FIXTURE: &str = include_str!("../../tests/fixtures/cquery-transitions.txt");

    fn dep_pairs(parsed: &TargetTransitions) -> Vec<(&str, Option<&str>)> {
        parsed
            .deps
            .iter()
            .map(|dep| (dep.label.as_str(), dep.transition.as_deref()))
            .collect()
    }

    #[test]
    fn transitions_fixture_reads_only_the_selected_root() {
        // The fixture lists //app:tool and //app:input.txt headers before
        // //app:chosen; only the chosen header's own rows become deps.
        let parsed = parse_transitions(
            TRANSITIONS_FIXTURE.as_bytes(),
            "//app:chosen",
            Some("7f8856bdeadbeef"),
        )
        .unwrap();
        assert!(!parsed.deps_truncated);
        assert_eq!(
            dep_pairs(&parsed),
            [
                ("//app:input.txt", None),
                ("//app:tool", Some("exec")),
                ("@bazel_tools//tools/genrule:genrule-setup.sh", None),
            ]
        );
    }

    #[test]
    fn transitions_fixture_supports_tool_and_null_config_headers() {
        let tool = parse_transitions(
            TRANSITIONS_FIXTURE.as_bytes(),
            "//app:tool",
            Some("f50cc19deadbeef"),
        )
        .unwrap();
        assert_eq!(
            dep_pairs(&tool),
            [("@bazel_tools//tools/genrule:genrule-setup.sh", None)]
        );

        let source =
            parse_transitions(TRANSITIONS_FIXTURE.as_bytes(), "//app:input.txt", None).unwrap();
        assert!(source.deps.is_empty());
    }

    #[test]
    fn transitions_rejects_a_missing_root_header() {
        assert!(
            parse_transitions(TRANSITIONS_FIXTURE.as_bytes(), "//app:missing", Some("abc"))
                .is_err()
        );
        // A null-configuration target rejects a configured checksum too.
        assert!(
            parse_transitions(
                TRANSITIONS_FIXTURE.as_bytes(),
                "//app:input.txt",
                Some("abc")
            )
            .is_err()
        );
    }

    #[test]
    fn transitions_ignore_same_label_headers_under_other_configurations() {
        // deps(config(...)) scopes to one configuration, but retained
        // same-label headers can still appear; only the checksum-matched
        // header collects rows — wrong configurations before AND after.
        let input = "\
NoTransition -> //app:a (aaa1111)
  deps#//app:stale#NoTransition ->
NoTransition -> //app:b (bbb2222)
  deps#//app:other#NoTransition ->
NoTransition -> //app:a (2d89340)
  deps#//app:live#(exec + (Trim)) -> ddd4444
NoTransition -> //app:a (ccc3333)
  deps#//app:later#NoTransition ->
";
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("2d89340cafe")).unwrap();
        assert_eq!(dep_pairs(&parsed), [("//app:live", Some("exec"))]);
    }

    #[test]
    fn transitions_dedupe_rows_and_ignore_later_headers() {
        let input = "\
NoTransition -> //app:a (aaa1111)
  deps#//app:shared#(exec + (Trim)) -> bbb2222
  deps#//app:shared#(exec + (Trim)) -> bbb2222
  srcs#//app:one#NoTransition ->
  data#//app:two#(null transition) ->
NoTransition -> //app:b (bbb2222)
  deps#//app:other#(null transition) ->
NoTransition -> //app:a (aaa1111)
  deps#//app:stale#(null transition) ->
";
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        // Duplicate rows collapse; once collection ends no later header —
        // even an identical one — resumes it.
        assert_eq!(
            dep_pairs(&parsed),
            [
                ("//app:shared", Some("exec")),
                ("//app:one", None),
                ("//app:two", None),
            ]
        );
    }

    #[test]
    fn transition_rows_keep_hashes_inside_labels() {
        let input = "NoTransition -> //app:a (aaa1111)\n  deps#//app:x#y#(null transition) ->\n";
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        assert_eq!(parsed.deps[0].label, "//app:x#y");
        assert_eq!(parsed.deps[0].transition, None);
    }

    #[test]
    fn transition_rows_stop_at_the_dependency_cap() {
        let mut input = String::from("NoTransition -> //app:a (aaa1111)\n");
        for i in 0..MAX_CONFIGURED_DEPS + 5 {
            input.push_str(&format!("  deps#//app:d{i}#NoTransition ->\n"));
        }
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        assert_eq!(parsed.deps.len(), MAX_CONFIGURED_DEPS);
        assert!(parsed.deps_truncated);
        // Rows past the cap are ignored entirely, duplicates included.
        input.push_str("  deps#//app:d0#NoTransition ->\n");
        let reparsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        assert_eq!(reparsed.deps.len(), MAX_CONFIGURED_DEPS);
    }

    #[test]
    fn transition_badge_marks_only_exec_and_keeps_other_names_whole() {
        assert_eq!(transition_badge("(null transition)"), None);
        assert_eq!(transition_badge("NoTransition"), None);
        assert_eq!(transition_badge("exec"), Some("exec".to_string()));
        assert_eq!(
            transition_badge(
                "(exec + (TestTrimmingTransition + ConfigFeatureFlagTaggedTrimmingTransition))"
            ),
            Some("exec".to_string())
        );
        // Non-exec transition names stay whole so the badge stays honest.
        let starlark =
            "(Starlark transition:/ws/app/transition.bzl:4:40 + (TestTrimmingTransition))";
        assert_eq!(transition_badge(starlark), Some(starlark.to_string()));
    }

    #[test]
    fn configuration_lookup_matches_by_id_not_position() {
        // The matching configuration is the second entry; options payloads
        // are present but never materialized into the summary.
        let json = r#"{
            "results": [
                {"target": {"type": "RULE"}, "configurationId": 2,
                 "configuration": {"checksum": "bbbb"}}
            ],
            "configurations": [
                {"id": 1, "mnemonic": "k8-fastbuild",
                 "checksum": "aaaaaaaaaaaaaaaa",
                 "options": {"compilation_mode": "fastbuild", "huge": "payload"},
                 "platformName": "@//:host"},
                {"id": 2, "mnemonic": "k8-opt",
                 "checksum": "bbbbbbbbbbbbbbbb",
                 "options": {"compilation_mode": "opt"}}
            ]
        }"#;
        let config = parse_configuration(std::io::Cursor::new(json))
            .unwrap()
            .unwrap();
        assert_eq!(config.mnemonic, "k8-opt");
        assert_eq!(config.checksum, "bbbbbbbbbbbbbbbb");
    }

    #[test]
    fn configuration_lookup_handles_null_and_rejects_ambiguity() {
        // No configurationId (source file) means the null configuration.
        let null = parse_configuration(std::io::Cursor::new(
            r#"{"results": [{"target": {"type": "SOURCE_FILE"}}], "configurations": []}"#,
        ))
        .unwrap();
        assert!(null.is_none());
        // An explicit zero id is the same thing.
        let zero = parse_configuration(std::io::Cursor::new(
            r#"{"results": [{"configurationId": 0}], "configurations": []}"#,
        ))
        .unwrap();
        assert!(zero.is_none());
        // Not exactly one result, or an id missing from configurations, fails.
        assert!(
            parse_configuration(std::io::Cursor::new(
                r#"{"results": [], "configurations": []}"#
            ))
            .is_err()
        );
        assert!(
            parse_configuration(std::io::Cursor::new(
                r#"{"results": [{"configurationId": 1}, {"configurationId": 2}], "configurations": []}"#
            ))
            .is_err()
        );
        assert!(
            parse_configuration(std::io::Cursor::new(
                r#"{"results": [{"configurationId": 9}], "configurations": [{"id": 1, "mnemonic": "m", "checksum": "c"}]}"#
            ))
            .is_err()
        );
    }

    #[test]
    fn configured_phases_reject_output_flag_overrides_before_launching() {
        // build_flags runs before any fetch inside configured_target and
        // target_actions, so an --output override can never reach a phase.
        for (kind, flags) in [
            (crate::query::QueryKind::Cquery, "--output=files"),
            (crate::query::QueryKind::Cquery, "--output files"),
            (crate::query::QueryKind::Cquery, "--output_file=x"),
            (crate::query::QueryKind::Aquery, "--output=files"),
            (crate::query::QueryKind::Aquery, "--output files"),
            (crate::query::QueryKind::Aquery, "--output_file=x"),
        ] {
            assert!(crate::query::build_flags(kind, flags).is_err(), "{flags:?}");
        }
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn target_args_order_user_flags_managed_options_and_expression() {
        let flags = strings(&["--config=opt", "--keep_going"]);
        assert_eq!(
            target_args(
                "cquery",
                &flags,
                &["--output=transitions", "--transitions=lite"],
                "deps(\"//app:t\", 1)"
            ),
            strings(&[
                "--noblock_for_lock",
                "cquery",
                "--config=opt",
                "--keep_going",
                "--output=transitions",
                "--transitions=lite",
                "--",
                "deps(\"//app:t\", 1)",
            ])
        );
        // The jsonproto configuration read scopes to the `config()` root.
        assert_eq!(
            target_args(
                "cquery",
                &flags,
                &["--output=jsonproto", "--proto:include_configurations"],
                "config(\"//app:t\", target)"
            ),
            strings(&[
                "--noblock_for_lock",
                "cquery",
                "--config=opt",
                "--keep_going",
                "--output=jsonproto",
                "--proto:include_configurations",
                "--",
                "config(\"//app:t\", target)",
            ])
        );
    }

    #[test]
    fn action_read_quotes_the_label_and_keeps_managed_options() {
        // The same arguments target_actions hands to TargetReader::read.
        let args = target_args(
            "aquery",
            &strings(&["--config=opt"]),
            &ACTION_MANAGED,
            "\"//app:t\"",
        );
        assert_eq!(
            args,
            strings(&[
                "--noblock_for_lock",
                "aquery",
                "--config=opt",
                "--output=jsonproto",
                "--include_commandline",
                "--include_artifacts",
                "--",
                "\"//app:t\"",
            ])
        );
    }

    #[test]
    fn config_summary_serializes_only_mnemonic_and_checksum() {
        let summary = ConfigSummary {
            mnemonic: "k8-opt".to_string(),
            checksum: "abc123".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&summary).unwrap(),
            serde_json::json!({
                "mnemonic": "k8-opt",
                "checksum": "abc123",
            })
        );
    }
}
