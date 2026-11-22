//! Ad-hoc Bazel graph queries for the query editor.
//!
//! Distinct from ingest's fixed `bazel query //...` snapshot fetch: the
//! expression and the output format come from the editor. Execution registers
//! its own owned invocation like every other Bazel operation, and keeps
//! bounded output: the first bytes of stdout and the tail of stderr.
//!
//! Composer Stop and app exit use the same process-group lifecycle as other
//! Bazel operations; queries have no separate timeout.

use crate::bazel_output::{capture_head, capture_tail};
use crate::runner::{self, Output};
use crate::state::Core;
use crate::{actions, runargs};
use serde::{Deserialize, Serialize};
use std::io::BufReader;
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tauri::AppHandle;

/// Cap on the expression text: far beyond a hand-built query.
const MAX_EXPR_LEN: usize = 100_000;

/// First bytes of stdout kept for the result; the rest is drained and dropped.
const STDOUT_CAP: usize = 4 * 1024 * 1024;
const AQUERY_JSON_CAP: usize = 32 * 1024 * 1024;
const MAX_FLAGS_LEN: usize = 16 * 1024;
const MAX_FLAGS: usize = 128;

/// stderr keeps only its tail: bazel writes its errors there.
const STDERR_TAIL: usize = 8 * 1024;

/// Generous bound for a confirmed exit's reader join: buffered output drains
/// in milliseconds, so anything past this is a lingering descendant.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryKind {
    Query,
    Cquery,
    Aquery,
}

impl QueryKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Cquery => "cquery",
            Self::Aquery => "aquery",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub workspace: String,
    pub kind: QueryKind,
    pub expr: String,
    pub output: String,
    #[serde(default)]
    pub no_implicit_deps: bool,
    #[serde(default)]
    pub no_tool_deps: bool,
    #[serde(default)]
    pub flags: String,
}

/// Result of one ad-hoc query invocation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryOutput {
    /// Exit code, or `None` when the process exited from a signal.
    pub exit_code: Option<i32>,
    /// stdout, at most the first [`STDOUT_CAP`] bytes.
    pub stdout: String,
    /// stdout exceeded the cap; only the first bytes are kept.
    pub truncated: bool,
    /// The last ~8 KiB of stderr, where bazel writes its errors.
    pub stderr_tail: String,
    pub actions: Option<actions::TargetActions>,
}

/// User flags precede the managed output settings so rc configs expand before
/// them. Standalone flags must not consume the following argument as a value.
/// Shared by the ad-hoc query runner and the inspector's configured-target
/// reads so both enforce the same flag policy.
pub(crate) fn build_flags(kind: QueryKind, flags: &str) -> Result<Vec<String>, String> {
    if flags.len() > MAX_FLAGS_LEN {
        return Err("build flags exceed 16 KiB".into());
    }
    if flags.chars().any(|c| c.is_control() && c != '\t') {
        return Err("build flags contain control characters".into());
    }
    if kind == QueryKind::Query && !flags.trim().is_empty() {
        return Err("query does not accept build flags".into());
    }
    let flags = runargs::parse_arguments(flags)?;
    if flags.len() > MAX_FLAGS {
        return Err("build flags exceed 128 tokens".into());
    }
    for flag in &flags {
        let name = flag.split_once('=').map_or(flag.as_str(), |(name, _)| name);
        if !name.starts_with('-') || name.len() < 2 {
            return Err("expected a build flag in --flag=value form".into());
        }
        if matches!(
            name,
            "--" | "--output"
                | "--output_file"
                | "--starlark:file"
                | "--starlark:expr"
                | "--query_file"
                | "--include_commandline"
                | "--noinclude_commandline"
                | "--include_artifacts"
                | "--noinclude_artifacts"
                | "--transitions"
                | "--notransitions"
                | "--proto:include_configurations"
                | "--noproto:include_configurations"
        ) {
            return Err("build flags must not override query options".into());
        }
        if !flag.contains('=')
            && !matches!(
                name,
                "--keep_going"
                    | "--nokeep_going"
                    | "--verbose_failures"
                    | "--noremote_accept_cached"
                    | "--noremote_upload_local_results"
            )
        {
            return Err("use --flag=value unless the flag is an allowed boolean switch".into());
        }
    }
    Ok(flags)
}

fn argv(req: &QueryRequest) -> Result<Vec<String>, String> {
    validate_expr(&req.expr)?;
    let mut args = vec!["--noblock_for_lock".into(), req.kind.as_str().into()];
    args.extend(build_flags(req.kind, &req.flags)?);
    args.push(format!("--output={}", req.output));
    match (req.kind, req.output.as_str()) {
        (QueryKind::Query, "label" | "graph") => {
            if req.output == "graph" {
                args.push("--nograph:factored".into());
            }
            if req.no_implicit_deps {
                args.push("--noimplicit_deps".into());
            }
            if req.no_tool_deps {
                args.push("--notool_deps".into());
            }
        }
        (QueryKind::Cquery, "label") => args.push("--show_config_fragments=off".into()),
        (QueryKind::Cquery, "transitions") => args.push("--transitions=lite".into()),
        (QueryKind::Aquery, "jsonproto") => {
            args.push("--include_commandline".into());
            args.push("--include_artifacts".into());
        }
        (QueryKind::Aquery, "text") => {}
        _ => return Err("unsupported output for selected query mode".into()),
    }
    args.push("--".into());
    args.push(req.expr.clone());
    Ok(args)
}

/// A leading `-` would be read as a bazel flag rather than an expression, and
/// control characters other than tab, newline and return have no place in a
/// query. Messages do not echo the expression.
fn validate_expr(expr: &str) -> Result<(), String> {
    if expr.trim().is_empty() {
        return Err("query expression is empty".to_string());
    }
    if expr.len() > MAX_EXPR_LEN {
        return Err("query expression is too long".to_string());
    }
    if expr.trim_start().starts_with('-') {
        return Err("query expression must not start with '-'".to_string());
    }
    if expr
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return Err("query expression contains control characters".to_string());
    }
    Ok(())
}

/// Run one ad-hoc query. Blocks until the query exits or its
/// stop is given up on.
#[tracing::instrument(target = "zeal::operations", name = "query.exec", skip_all)]
pub fn run(app: AppHandle, core: Arc<Core>, request: &QueryRequest) -> Result<QueryOutput, String> {
    let (mut child, id) = spawn_query(&core, request)?;
    let _slot = runner::InvocationGuard {
        core: &core,
        id: &id,
        emit: || core.emit(&app),
    };
    core.emit(&app);
    // stdout and stderr are drained concurrently so the child never blocks on
    // a full pipe; each reader sends its capture once, at end of stream.
    let (out_tx, out_rx) = mpsc::channel();
    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let json = request.kind == QueryKind::Aquery && request.output == "jsonproto";
    let cap = if json { AQUERY_JSON_CAP } else { STDOUT_CAP };
    thread::spawn(move || {
        let _ = out_tx.send(capture_head(&mut out_pipe, cap));
    });
    let (err_tx, err_rx) = mpsc::channel();
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    thread::spawn(move || {
        let _ = err_tx.send(capture_tail(&mut err_pipe, STDERR_TAIL));
    });
    let exit = runner::wait_query(&core, &id, &mut child)?;
    // A starved reader must not fake an empty result: dropped stdout is
    // reported as truncated.
    let (stdout, truncated) = match drain(&out_rx)
        .transpose()
        .map_err(|error| format!("failed to read query stdout: {error}"))?
    {
        Some((bytes, dropped)) => (bytes, dropped),
        None => (Vec::new(), true),
    };
    let stderr = drain(&err_rx)
        .transpose()
        .map_err(|error| format!("failed to read query stderr: {error}"))?;
    let stderr_tail = String::from_utf8_lossy(stderr.as_deref().unwrap_or_default()).into_owned();
    query_output(
        json,
        (exit >= 0).then_some(exit),
        &stdout,
        truncated,
        stderr_tail,
    )
}

fn query_output(
    json: bool,
    exit_code: Option<i32>,
    stdout: &[u8],
    truncated: bool,
    stderr_tail: String,
) -> Result<QueryOutput, String> {
    let actions = if json && !truncated && exit_code == Some(0) {
        Some(actions::parse(BufReader::new(stdout))?)
    } else {
        None
    };
    Ok(QueryOutput {
        exit_code,
        stdout: if json {
            String::new()
        } else {
            String::from_utf8_lossy(stdout).into_owned()
        },
        truncated,
        stderr_tail,
        actions,
    })
}

/// Register the query as its own owned invocation and spawn it atomically.
fn spawn_query(core: &Core, req: &QueryRequest) -> Result<(Child, String), String> {
    let args = argv(req)?;
    let mut st = core.state.lock().expect("application state lock poisoned");
    let ws_path = st.workspace_path(&req.workspace)?;
    let bazel = st.bazel.clone().ok_or_else(|| "no bazel".to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut command = Command::new(&bazel);
    command.args(args).current_dir(&ws_path);
    let (child, _) = runner::spawn(
        &mut st,
        &mut command,
        runner::InvocationMeta {
            workspace: &ws_path,
            cmd: req.kind.as_str(),
            pattern: &req.expr,
            id: &id,
        },
        Output::Pipes,
    )
    .map_err(|error| format!("failed to spawn {}: {error}", bazel.display()))?;
    Ok((child, id))
}

/// Receive a reader's capture. Even a confirmed exit is bounded: a descendant
/// still holding the pipe would otherwise wedge the bazel slot forever — the
/// same hazard runner bounds with its drain grace. The confirmed wait is the
/// longer `DRAIN_GRACE` so ordinary end-of-stream delivery is not raced.
/// `None` means the reader never delivered within the bound; callers must
/// surface that loss rather than report a fabricated empty capture.
fn drain<T>(rx: &mpsc::Receiver<T>) -> Option<T> {
    rx.recv_timeout(DRAIN_GRACE).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{self, Read};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn request(kind: QueryKind, output: &str) -> QueryRequest {
        serde_json::from_value(serde_json::json!({
            "kind": kind.as_str(), "workspace": "/workspace", "output": output,
            "expr": "//app:chosen",
        }))
        .unwrap()
    }

    #[test]
    fn query_arguments_put_the_expression_last() {
        let req = request(QueryKind::Query, "label");
        assert_eq!(
            argv(&req).unwrap(),
            [
                "--noblock_for_lock",
                "query",
                "--output=label",
                "--",
                "//app:chosen"
            ]
        );
        let req = QueryRequest {
            output: "graph".into(),
            no_implicit_deps: true,
            no_tool_deps: true,
            ..req
        };
        assert_eq!(
            argv(&req).unwrap(),
            [
                "--noblock_for_lock",
                "query",
                "--output=graph",
                "--nograph:factored",
                "--noimplicit_deps",
                "--notool_deps",
                "--",
                "//app:chosen"
            ]
        );
    }

    #[test]
    fn configured_queries_fix_the_output_after_user_flags() {
        for (kind, output, managed) in [
            (
                QueryKind::Cquery,
                "label",
                &["--show_config_fragments=off"][..],
            ),
            (QueryKind::Cquery, "transitions", &["--transitions=lite"]),
            (
                QueryKind::Aquery,
                "jsonproto",
                &["--include_commandline", "--include_artifacts"],
            ),
            (QueryKind::Aquery, "text", &[]),
        ] {
            let req = QueryRequest {
                flags: "--config=opt".into(),
                ..request(kind, output)
            };
            let output_flag = format!("--output={output}");
            let mut expected = vec![
                "--noblock_for_lock",
                kind.as_str(),
                "--config=opt",
                &output_flag,
            ];
            expected.extend_from_slice(managed);
            expected.extend(["--", "//app:chosen"]);
            assert_eq!(argv(&req).unwrap(), expected);
        }
    }

    #[test]
    fn rejects_managed_flags_residue_missing_values_and_unbounded_input() {
        let mut req = request(QueryKind::Aquery, "jsonproto");
        for flag in [
            "--",
            "-",
            "x",
            "-=x",
            "=x",
            "--=x",
            "--tool_tag",
            "--platforms",
            "''",
            "--output=proto",
            "--output_file=x",
            "--starlark:file=x",
            "--starlark:expr=x",
            "--query_file=x",
            "--include_commandline=false",
            "--noinclude_commandline",
            "--include_artifacts",
            "--noinclude_artifacts",
            "--transitions=x",
            "--notransitions",
            "--proto:include_configurations=x",
            "--noproto:include_configurations=x",
            "--config=a residue",
            "--config=\0",
            "--config=a\n",
            "\r",
            "--config='",
        ] {
            req.flags = flag.into();
            assert!(argv(&req).is_err(), "{flag:?}");
        }
        req.flags = "--keep_going ".repeat(MAX_FLAGS);
        assert!(argv(&req).is_ok());
        req.flags.push_str("--keep_going");
        assert!(argv(&req).is_err());
        req.flags = format!("--x={}", "a".repeat(MAX_FLAGS_LEN - 4));
        assert!(argv(&req).is_ok());
        req.flags.push('a');
        assert!(argv(&req).is_err());
        req = request(QueryKind::Query, "label");
        req.flags = "--config=opt".into();
        assert!(argv(&req).is_err());
    }

    #[test]
    fn accepts_explicit_values_and_known_boolean_switches() {
        let mut req = request(QueryKind::Cquery, "label");
        req.flags = "--output_groups=foo\t--output_filter='a b' --config=opt --keep_going --nokeep_going --verbose_failures --noremote_accept_cached --noremote_upload_local_results".into();
        assert_eq!(
            build_flags(req.kind, &req.flags).unwrap(),
            [
                "--output_groups=foo",
                "--output_filter=a b",
                "--config=opt",
                "--keep_going",
                "--nokeep_going",
                "--verbose_failures",
                "--noremote_accept_cached",
                "--noremote_upload_local_results"
            ]
        );
    }

    #[test]
    fn expressions_reject_empty_dash_prefix_control_and_overflow() {
        for expr in [
            "",
            "   ",
            "\t\n",
            "-deps(//a:b)",
            "  -x",
            "a\0b",
            "a\u{7f}b",
            "a\u{9f}b",
        ] {
            assert!(validate_expr(expr).is_err(), "{expr:?}");
        }
        assert!(validate_expr(&"a".repeat(MAX_EXPR_LEN + 1)).is_err());
    }

    #[test]
    fn expressions_accept_queries_whitespace_and_the_length_bound() {
        for expr in [
            "deps(//a:b)",
            "//...",
            "a\tb",
            "deps(//a)\nunion\ndeps(//b)",
            "a\rb",
        ] {
            assert!(validate_expr(expr).is_ok(), "{expr:?}");
        }
        assert!(validate_expr(&"a".repeat(MAX_EXPR_LEN)).is_ok());
    }

    #[test]
    fn rejects_formats_for_other_query_kinds() {
        for (kind, output) in [
            (QueryKind::Query, "transitions"),
            (QueryKind::Cquery, "graph"),
            (QueryKind::Aquery, "label"),
            (QueryKind::Query, "Label"),
            (QueryKind::Aquery, ""),
        ] {
            assert!(argv(&request(kind, output)).is_err());
        }
    }

    #[test]
    fn validation_errors_name_the_rule_not_the_expression() {
        // Error strings name the rule, never the expression.
        let msg = validate_expr("-deps(//secret:t)").unwrap_err();
        assert!(!msg.contains("secret"), "{msg}");
        let msg = validate_expr("bad\0expr").unwrap_err();
        assert!(!msg.contains("bad"), "{msg}");
    }

    fn assert_query_entry(st: &crate::state::CoreState, id: &str, ws: &str, kind: QueryKind) {
        let invocation = st.invocations.get(id).unwrap();
        assert_eq!(invocation.cmd, kind.as_str());
        assert_eq!(invocation.workspace, PathBuf::from(ws));
        let process = &invocation.process;
        assert!(process.terminal.is_none());
        assert!(process.pgid.is_some());
    }

    fn assert_reap_state(st: &crate::state::CoreState, reaped: &str, other: &str) {
        // The reap revokes signal ownership; the entry releases at guard drop.
        let process = &st.invocations[reaped].process;
        assert!(process.pgid.is_none());
        assert!(st.invocations.contains_key(other));
    }

    fn assert_slot_released(st: &crate::state::CoreState, released: &str, held: &str) {
        // Release removes the entry entirely.
        assert!(!st.invocations.contains_key(released));
        assert!(st.invocations.contains_key(held));
    }

    #[test]
    fn spawned_queries_each_hold_their_own_invocation() {
        let dir = std::env::temp_dir().join(format!("zeal-query-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let workspace = dir.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let bazel = dir.join("bazel");
        fs::write(&bazel, "#!/bin/sh\nprintf '//app:chosen\\n'\n").unwrap();
        fs::set_permissions(&bazel, fs::Permissions::from_mode(0o700)).unwrap();

        let core = Core::new();
        let ws = workspace.to_str().unwrap().to_string();
        let mut st = core.state.lock().unwrap();
        st.workspaces
            .insert(workspace, crate::scope::Scope::default());
        st.bazel = Some(bazel);
        drop(st);
        // Queries never serialize each other: both register before either ends.
        let a = QueryRequest {
            workspace: ws.clone(),
            ..request(QueryKind::Cquery, "label")
        };
        let b = QueryRequest {
            workspace: ws.clone(),
            ..request(QueryKind::Aquery, "text")
        };
        let (mut child_a, id_a) = spawn_query(&core, &a).unwrap();
        let (mut child_b, id_b) = spawn_query(&core, &b).unwrap();
        let st = core.state.lock().unwrap();
        for (id, kind) in [(&id_a, a.kind), (&id_b, b.kind)] {
            assert_query_entry(&st, id, &ws, kind);
        }
        drop(st);
        let _guard_a = runner::InvocationGuard {
            core: &core,
            id: &id_a,
            emit: || (),
        };
        assert_eq!(runner::wait_query(&core, &id_a, &mut child_a).unwrap(), 0);
        {
            let st = core.state.lock().unwrap();
            assert_reap_state(&st, &id_a, &id_b);
        }
        drop(_guard_a);
        {
            let st = core.state.lock().unwrap();
            assert_slot_released(&st, &id_a, &id_b);
        }

        let _guard_b = runner::InvocationGuard {
            core: &core,
            id: &id_b,
            emit: || (),
        };
        assert_eq!(runner::wait_query(&core, &id_b, &mut child_b).unwrap(), 0);
        drop(_guard_b);
        fs::remove_dir_all(dir).unwrap();
    }

    /// The wire shape the frontend's `QueryOutput` consumes.
    #[test]
    fn query_output_serializes_in_camel_case() {
        let json = serde_json::to_value(QueryOutput {
            exit_code: Some(2),
            stdout: "//a:b\n".to_string(),
            truncated: false,
            stderr_tail: "oops".to_string(),
            actions: None,
        })
        .unwrap();
        assert_eq!(json["exitCode"], 2);
        assert_eq!(json["stderrTail"], "oops");
        assert!(json.get("stderr_tail").is_none());
    }

    #[test]
    fn jsonproto_parses_only_an_intact_successful_capture() {
        let valid = br#"{"actions":[{"targetId":1,"mnemonic":"Genrule"}],"targets":[{"id":1,"label":"//app:chosen"}]}"#;
        let out = query_output(true, Some(0), valid, false, String::new()).unwrap();
        assert_eq!(out.stdout, "");
        assert_eq!(out.actions.unwrap().actions[0].owner, "//app:chosen");
        assert!(query_output(true, Some(0), b"bad json", false, String::new()).is_err());
    }

    #[test]
    fn incomplete_jsonproto_returns_status_and_diagnostics() {
        for (exit, truncated) in [(Some(1), false), (None, false), (Some(0), true)] {
            let out = query_output(true, exit, b"partial", truncated, "error".into()).unwrap();
            assert!(out.actions.is_none());
            assert_eq!(out.stdout, "");
            assert_eq!(out.exit_code, exit);
            assert_eq!(out.stderr_tail, "error");
            assert_eq!(out.truncated, truncated);
        }
    }

    struct InterruptOnce<R> {
        inner: R,
        interruption: Option<io::Error>,
    }

    impl<R: Read> Read for InterruptOnce<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.interruption
                .take()
                .map_or_else(|| self.inner.read(buf), Err)
        }
    }

    struct BrokenPipe;

    impl Read for BrokenPipe {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }

    #[test]
    fn captures_keep_the_stdout_head_and_the_stderr_tail() {
        let data = "0123456789".repeat(2000); // 20_000 bytes
        let (head, truncated) = capture_head(&mut data.as_bytes(), 4096).unwrap();
        assert_eq!(head, data.as_bytes()[..4096]);
        assert!(truncated);

        let tail = capture_tail(&mut data.as_bytes(), 100).unwrap();
        assert_eq!(tail, data.as_bytes()[data.len() - 100..]);

        let small = "short";
        let (head, truncated) = capture_head(&mut small.as_bytes(), 4096).unwrap();
        assert_eq!(head, b"short");
        assert!(!truncated);
    }

    #[test]
    fn interrupted_reads_retry_and_keep_the_capture() {
        let data = "0123456789".repeat(2000);
        let mut head = InterruptOnce {
            inner: data.as_bytes(),
            interruption: Some(io::ErrorKind::Interrupted.into()),
        };
        let (bytes, truncated) = capture_head(&mut head, 4096).unwrap();
        assert_eq!(bytes, data.as_bytes()[..4096]);
        assert!(truncated);

        let mut tail = InterruptOnce {
            inner: data.as_bytes(),
            interruption: Some(io::ErrorKind::Interrupted.into()),
        };
        assert_eq!(
            capture_tail(&mut tail, 100).unwrap(),
            data.as_bytes()[data.len() - 100..]
        );
    }

    #[test]
    fn read_errors_surface_instead_of_a_fabricated_capture() {
        let head = capture_head(&mut BrokenPipe, 4096).unwrap_err();
        assert_eq!(head.kind(), io::ErrorKind::BrokenPipe);
        let tail = capture_tail(&mut BrokenPipe, 100).unwrap_err();
        assert_eq!(tail.kind(), io::ErrorKind::BrokenPipe);
    }
}
