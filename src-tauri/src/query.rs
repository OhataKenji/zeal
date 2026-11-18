//! Ad-hoc `bazel query` for the query editor.
//!
//! Distinct from ingest's fixed `bazel query //...` snapshot fetch: the
//! expression and the output format come from the editor. Execution registers
//! its own owned invocation like every other Bazel operation, and keeps
//! bounded output: the first bytes of stdout and the tail of stderr.
//!
//! Composer Stop and app exit use the same process-group lifecycle as other
//! Bazel operations; queries have no separate timeout.

use crate::runner::{self, Output};
use crate::state::Core;
use serde::Serialize;
use std::io::{self, Read};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tauri::AppHandle;

/// The only output formats the editor offers.
const OUTPUTS: [&str; 2] = ["label", "graph"];

/// Cap on the expression text: far beyond a hand-built query.
const MAX_EXPR_LEN: usize = 100_000;

/// First bytes of stdout kept for the result; the rest is drained and dropped.
const STDOUT_CAP: usize = 4 * 1024 * 1024;

/// stderr keeps only its tail: bazel writes its errors there.
const STDERR_TAIL: usize = 8 * 1024;

/// Generous bound for a confirmed exit's reader join: buffered output drains
/// in milliseconds, so anything past this is a lingering descendant.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Result of one ad-hoc `bazel query` invocation.
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
}

/// Arguments after the bazel executable. The expression is one `Command::arg`:
/// nothing is split, quoted or escaped. No `--tool_tag` (query versions may
/// reject it) and no `--keep_going` (partial results would be unusable).
fn argv(expr: &str, output: &str, no_implicit: bool, no_tool: bool) -> Vec<String> {
    let mut args = vec![
        // A startup flag, before the subcommand: a bazel already holding the
        // server lock fails this client immediately instead of looking hung.
        "--noblock_for_lock".to_string(),
        "query".to_string(),
        expr.to_string(),
        format!("--output={output}"),
    ];
    // Factored keeps one node per target instead of merging equal ones.
    if output == "graph" {
        args.push("--nograph:factored".to_string());
    }
    if no_implicit {
        args.push("--noimplicit_deps".to_string());
    }
    if no_tool {
        args.push("--notool_deps".to_string());
    }
    args
}

/// Only the formats the editor can render.
fn validate_output(output: &str) -> Result<(), String> {
    if OUTPUTS.contains(&output) {
        Ok(())
    } else {
        Err("output must be \"label\" or \"graph\"".to_string())
    }
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

/// Run one ad-hoc `bazel query` in `ws`. Blocks until the query exits or its
/// stop is given up on.
#[tracing::instrument(target = "zeal::operations", name = "query.exec", skip_all)]
pub fn run(
    app: AppHandle,
    core: Arc<Core>,
    ws: &str,
    expr: &str,
    output: &str,
    no_implicit: bool,
    no_tool: bool,
) -> Result<QueryOutput, String> {
    validate_expr(expr)?;
    validate_output(output)?;
    let (mut child, id) = spawn_query(&core, ws, expr, output, no_implicit, no_tool)?;
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
    thread::spawn(move || {
        let _ = out_tx.send(capture_head(&mut out_pipe, STDOUT_CAP));
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
    Ok(QueryOutput {
        exit_code: (exit >= 0).then_some(exit),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        truncated,
        stderr_tail,
    })
}

/// Register the query as its own owned invocation and spawn it atomically.
fn spawn_query(
    core: &Core,
    ws: &str,
    expr: &str,
    output: &str,
    no_implicit: bool,
    no_tool: bool,
) -> Result<(Child, String), String> {
    let mut st = core.state.lock().expect("application state lock poisoned");
    let ws_path = st.workspace_path(ws)?;
    let bazel = st.bazel.clone().ok_or_else(|| "no bazel".to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut command = Command::new(&bazel);
    command
        .args(argv(expr, output, no_implicit, no_tool))
        .current_dir(&ws_path);
    let (child, _) = runner::spawn(
        &mut st,
        &mut command,
        runner::InvocationMeta {
            workspace: &ws_path,
            cmd: "query",
            pattern: expr,
            id: &id,
        },
        Output::Pipes,
    )
    .map_err(|error| format!("failed to spawn {}: {error}", bazel.display()))?;
    Ok((child, id))
}

/// Keep the first `cap` bytes of the stream and drain the rest: the child
/// must never block on a full pipe. Returns the bytes and whether any were
/// dropped.
fn capture_head(pipe: &mut impl Read, cap: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut buf = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
            Ok(n) => {
                let kept = n.min(cap - buf.len());
                buf.extend_from_slice(&chunk[..kept]);
                truncated |= kept < n;
            }
        }
    }
    Ok((buf, truncated))
}

/// Keep the last `cap` bytes of the stream.
fn capture_tail(pipe: &mut impl Read, cap: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                buf.drain(..buf.len().saturating_sub(cap));
            }
        }
    }
    Ok(buf)
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
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    #[test]
    fn argv_has_the_expression_the_output_and_the_flag_switches() {
        assert_eq!(
            argv("deps(//a:b)", "label", false, false),
            [
                "--noblock_for_lock",
                "query",
                "deps(//a:b)",
                "--output=label"
            ]
            .map(String::from)
        );
        assert_eq!(
            argv("//...", "graph", false, false),
            [
                "--noblock_for_lock",
                "query",
                "//...",
                "--output=graph",
                "--nograph:factored"
            ]
            .map(String::from)
        );
        assert_eq!(
            argv("//a:b", "label", true, false),
            [
                "--noblock_for_lock",
                "query",
                "//a:b",
                "--output=label",
                "--noimplicit_deps"
            ]
            .map(String::from)
        );
        assert_eq!(
            argv("//a:b", "label", false, true),
            [
                "--noblock_for_lock",
                "query",
                "//a:b",
                "--output=label",
                "--notool_deps"
            ]
            .map(String::from)
        );
        assert_eq!(
            argv("//a:b", "graph", true, true),
            [
                "--noblock_for_lock",
                "query",
                "//a:b",
                "--output=graph",
                "--nograph:factored",
                "--noimplicit_deps",
                "--notool_deps",
            ]
            .map(String::from)
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
    fn outputs_accept_only_label_and_graph() {
        for output in ["xml", "", "Label"] {
            assert!(validate_output(output).is_err(), "{output:?}");
        }
        for output in ["label", "graph"] {
            assert!(validate_output(output).is_ok(), "{output:?}");
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

    fn assert_query_entry(st: &crate::state::CoreState, id: &str, ws: &str) {
        let invocation = st.invocations.get(id).unwrap();
        assert_eq!(invocation.cmd, "query");
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
        let (mut child_a, id_a) = spawn_query(&core, &ws, "//...", "label", false, false).unwrap();
        let (mut child_b, id_b) = spawn_query(&core, &ws, "//...", "label", false, false).unwrap();
        let st = core.state.lock().unwrap();
        for id in [&id_a, &id_b] {
            assert_query_entry(&st, id, &ws);
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
        })
        .unwrap();
        assert_eq!(json["exitCode"], 2);
        assert_eq!(json["stderrTail"], "oops");
        assert!(json.get("stderr_tail").is_none());
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
