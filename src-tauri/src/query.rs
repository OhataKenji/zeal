//! Ad-hoc `bazel query` for the query editor.
//!
//! Distinct from ingest's fixed `bazel query //...` snapshot fetch: the
//! expression and the output format come from the editor. Execution holds the
//! same serialized bazel slot as ingest and invocation, and keeps bounded
//! output:
//! the first bytes of stdout and the tail of stderr.
//!
//! Index, detail, and editor queries share process supervision and the
//! Composer's Stop action. A group whose exit cannot be confirmed still
//! releases the slot, reported with no exit code.

use crate::ingest::QUERY_TIMEOUT;
use crate::runner::{STOP_GRACE, signal_group};
use crate::state::{Core, CoreState};
use rustix::process::Signal;
use serde::Serialize;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// The only output formats the editor offers.
const OUTPUTS: [&str; 2] = ["label", "graph"];

/// Cap on the expression text: far beyond a hand-built query.
const MAX_EXPR_LEN: usize = 100_000;

/// First bytes of stdout kept for the result; the rest is drained and dropped.
const STDOUT_CAP: usize = 4 * 1024 * 1024;

/// stderr keeps only its tail: bazel writes its errors there.
const STDERR_TAIL: usize = 8 * 1024;

/// Time SIGKILL gets to end the query before its exit is called unconfirmed.
const KILL_CONFIRM: Duration = Duration::from_secs(2);

/// When the exit is unconfirmed, the reader join is bounded: a descendant
/// still holding a pipe must not delay the slot's release.
const READER_GRACE: Duration = Duration::from_millis(500);

/// Generous bound for a confirmed exit's reader join: buffered output drains
/// in milliseconds, so anything past this is a lingering descendant.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Poll interval of the wait loop.
const POLL: Duration = Duration::from_millis(50);

/// Appended to stderr when the group's death could not be confirmed.
const UNCONFIRMED_NOTE: &str = "Could not confirm the exit";

/// Result of one ad-hoc `bazel query` invocation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryOutput {
    /// Exit code, when the exit was confirmed and carries one. A signal death
    /// and an unconfirmed exit are both `None`.
    pub exit_code: Option<i32>,
    /// stdout, at most the first [`STDOUT_CAP`] bytes.
    pub stdout: String,
    /// stdout exceeded the cap; only the first bytes are kept.
    pub truncated: bool,
    /// The last ~8 KiB of stderr, where bazel writes its errors.
    pub stderr_tail: String,
    /// The deadline passed and the group was killed.
    pub timed_out: bool,
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

/// Run one ad-hoc `bazel query` in the open workspace. Busy requests — an
/// invocation, an ingest or another query in flight — are rejected rather than
/// queued.
/// Blocks until the query exits, times out, or is given up on.
#[tracing::instrument(target = "zeal::operations", name = "query.exec", skip_all)]
pub fn run(
    app: AppHandle,
    core: Arc<Core>,
    expr: &str,
    output: &str,
    no_implicit: bool,
    no_tool: bool,
) -> Result<QueryOutput, String> {
    validate_expr(expr)?;
    validate_output(output)?;
    let (ws, bazel) = acquire(&core)?;
    // Constructed before anything else can panic: unwinding releases the slot.
    let _slot = QuerySlot {
        core: &core,
        emit: || core.emit(&app),
    };
    core.emit(&app);
    // Its own process group: the timeout kill and app exit reach descendants.
    let mut child = match Command::new(&bazel)
        .args(argv(expr, output, no_implicit, no_tool))
        .current_dir(&ws)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return Err(format!("failed to spawn {}: {e}", bazel.display())),
    };
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
    let observed = wait(child, &core);
    if observed.cancelled {
        return Err(if observed.unconfirmed {
            UNCONFIRMED_NOTE.to_string()
        } else {
            "Query stopped".to_string()
        });
    }
    // A starved reader must not fake an empty result: dropped stdout is
    // reported as truncated and a missing stderr capture gets the same note
    // an unconfirmed exit does.
    let (stdout, truncated) = match drain(&out_rx, observed.unconfirmed)
        .transpose()
        .map_err(|error| format!("failed to read query stdout: {error}"))?
    {
        Some((bytes, dropped)) => (bytes, dropped),
        None => (Vec::new(), true),
    };
    let stderr = drain(&err_rx, observed.unconfirmed)
        .transpose()
        .map_err(|error| format!("failed to read query stderr: {error}"))?;
    let mut stderr_tail =
        String::from_utf8_lossy(stderr.as_deref().unwrap_or_default()).into_owned();
    if observed.unconfirmed || stderr.is_none() {
        if !stderr_tail.is_empty() {
            stderr_tail.push('\n');
        }
        stderr_tail.push_str(UNCONFIRMED_NOTE);
    }
    Ok(QueryOutput {
        exit_code: observed.exit,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        truncated,
        stderr_tail,
        timed_out: observed.timed_out,
    })
}

/// Take the serialized bazel slot for this query: reject while an
/// invocation, an ingest or another query holds it, and require an open
/// workspace with a resolved bazel.
fn acquire(core: &Core) -> Result<(PathBuf, PathBuf), String> {
    let mut st = core.state.lock().expect("application state lock poisoned");
    if st.bazel_busy() {
        return Err("bazel is busy; run the query after it finishes".to_string());
    }
    let (Some(ws), Some(bazel)) = (st.workspace.clone(), st.bazel.clone()) else {
        return Err("no workspace or bazel".to_string());
    };
    st.query_running = true;
    st.query_pgid = None;
    st.query_stopping = false;
    Ok((ws, bazel))
}

/// How supervising the query child ended.
pub(crate) struct Observed {
    /// Confirmed exit code (`None` for a signal death), if one was observed.
    pub(crate) exit: Option<i32>,
    /// The deadline passed; the group was killed.
    pub(crate) timed_out: bool,
    /// `wait` failed or the group outlived SIGKILL's confirm window.
    pub(crate) unconfirmed: bool,
    pub(crate) cancelled: bool,
}

/// Supervise any query while its caller owns the serialized Bazel slot.
pub(crate) fn wait(mut child: Child, core: &Core) -> Observed {
    let pgid = child.id() as i32;
    core.state
        .lock()
        .expect("application state lock poisoned")
        .query_pgid = Some(pgid);
    let observed = observe(&mut child, pgid, core, QUERY_TIMEOUT, KILL_CONFIRM, POLL);
    if observed.unconfirmed {
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
    observed
}

/// Poll the child until it exits or the deadline passes; the deadline sends
/// the group straight to SIGKILL, then allows a short confirm window before
/// calling the exit unconfirmed. Once `try_wait` reaps the child, its process
/// group is revoked immediately: the pid is no longer ours to signal.
#[tracing::instrument(target = "zeal::operations", name = "query.wait", skip_all)]
fn observe(
    child: &mut Child,
    pgid: i32,
    core: &Core,
    deadline: Duration,
    kill_confirm: Duration,
    poll: Duration,
) -> Observed {
    let end = Instant::now() + deadline;
    let mut confirm_end = None;
    let mut stop_end = None;
    let mut timed_out = false;
    loop {
        let mut st = core.state.lock().expect("application state lock poisoned");
        match child.try_wait() {
            Ok(Some(status)) => {
                clear_pgid(&mut st, pgid);
                return Observed {
                    exit: status.code(),
                    timed_out,
                    unconfirmed: false,
                    cancelled: st.query_stopping,
                };
            }
            Ok(None) => (),
            Err(_) => {
                // A failed wait can mean another reaper consumed the child;
                // its former pid is no longer safe to signal.
                clear_pgid(&mut st, pgid);
                return Observed {
                    exit: None,
                    timed_out,
                    unconfirmed: true,
                    cancelled: st.query_stopping,
                };
            }
        }
        if st.query_stopping && stop_end.is_none() {
            if let Err(error) = signal_group(pgid, Signal::INT) {
                log::warn!("SIGINT to query group failed: {error}");
            }
            stop_end = Some(Instant::now() + STOP_GRACE);
        }
        let kill_at = stop_end.map_or(end, |stop: Instant| stop.min(end));
        if confirm_end.is_none() && Instant::now() >= kill_at {
            timed_out = Instant::now() >= end;
        }
        if confirm_expired(&mut confirm_end, pgid, kill_at, kill_confirm) {
            clear_pgid(&mut st, pgid);
            return Observed {
                exit: None,
                timed_out,
                unconfirmed: true,
                cancelled: st.query_stopping,
            };
        }
        drop(st);
        thread::sleep(poll);
    }
}

/// Forget a reaped or killed group once its fate is known: the pid is no
/// longer ours to signal.
fn clear_pgid(st: &mut CoreState, pgid: i32) {
    if st.query_pgid == Some(pgid) {
        st.query_pgid = None;
    }
}

/// Once `end` passes the group gets one SIGKILL and `kill_confirm` to be
/// observed dead; `true` means the confirm window ran out.
fn confirm_expired(
    confirm_end: &mut Option<Instant>,
    pgid: i32,
    end: Instant,
    kill_confirm: Duration,
) -> bool {
    let now = Instant::now();
    if confirm_end.is_none() && now >= end {
        if let Err(e) = signal_group(pgid, Signal::KILL) {
            log::warn!("SIGKILL to query group failed: {e}");
        }
        *confirm_end = Some(now + kill_confirm);
    }
    confirm_end.is_some_and(|until| now >= until)
}

/// A panic in the worker must release the query slot. Only the holder can
/// release it: nothing outside `run` clears `query_running`, so the worker
/// still running provably owns the slot.
struct QuerySlot<'a, F: Fn()> {
    core: &'a Core,
    emit: F,
}

impl<F: Fn()> Drop for QuerySlot<'_, F> {
    fn drop(&mut self) {
        let mut st = self
            .core
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        st.query_running = false;
        st.query_pgid = None;
        st.query_stopping = false;
        drop(st);
        // Avoid a second panic from an event emission while unwinding.
        if !thread::panicking() {
            (self.emit)();
        }
    }
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
pub(crate) fn drain<T>(rx: &mpsc::Receiver<T>, unconfirmed: bool) -> Option<T> {
    let bound = if unconfirmed {
        READER_GRACE
    } else {
        DRAIN_GRACE
    };
    rx.recv_timeout(bound).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CoreState;
    use std::io::{BufRead, BufReader};

    /// Best-effort SIGKILL of the test's group, so a failed assertion cannot
    /// leave the `sleep` behind.
    struct GroupGuard(i32);

    impl Drop for GroupGuard {
        fn drop(&mut self) {
            let _ = signal_group(self.0, Signal::KILL);
        }
    }

    /// A `sleep 60` child in its own process group, ready for signals.
    fn sleeping(arg: &str) -> (Child, i32, GroupGuard) {
        let child = Command::new("sh")
            .arg("-c")
            .arg(arg)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id() as i32;
        (child, pgid, GroupGuard(pgid))
    }

    /// Wait until the fixture printed its readiness marker.
    fn wait_ready(child: &mut Child) {
        let mut ready = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
    }

    /// The state `run` leaves once its child is spawned.
    fn held(core: &Core, pgid: i32) {
        let mut st = core.state.lock().unwrap();
        st.query_running = true;
        st.query_pgid = Some(pgid);
    }

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

    /// Each way the slot can be held rejects a new query.
    #[test]
    fn acquire_rejects_while_any_bazel_invocation_holds_the_slot() {
        let core = Core::new();
        assert!(acquire(&core).is_err(), "no workspace or bazel");
        {
            let mut st = core.state.lock().unwrap();
            st.workspace = Some(PathBuf::from("/ws"));
            st.bazel = Some(PathBuf::from("/bin/bazel"));
        }
        let holds: [fn(&mut CoreState); 3] = [
            |st| st.ingest_running = true,
            |st| st.active_invocation = Some(crate::state::ActiveInvocation::stub("r", None)),
            |st| st.query_running = true,
        ];
        for hold in holds {
            hold(&mut core.state.lock().unwrap());
            assert!(acquire(&core).is_err());
            let mut st = core.state.lock().unwrap();
            st.ingest_running = false;
            st.active_invocation = None;
            st.query_running = false;
        }
    }

    /// A held query rejects a second one; the fields `run` leaves behind are
    /// all set at acquire.
    #[test]
    fn a_held_query_rejects_the_next_acquire() {
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.workspace = Some(PathBuf::from("/ws"));
            st.bazel = Some(PathBuf::from("/bin/bazel"));
            st.query_pgid = Some(42);
            st.query_stopping = true;
        }
        let (ws, bazel) = acquire(&core).unwrap();
        assert_eq!(ws, PathBuf::from("/ws"));
        assert_eq!(bazel, PathBuf::from("/bin/bazel"));
        let st = core.state.lock().unwrap();
        assert!(st.query_running);
        assert!(st.query_pgid.is_none());
        assert!(!st.query_stopping);
        drop(st);
        assert!(acquire(&core).is_err(), "a held query rejects the next");
    }

    /// The deadline sends the group straight to SIGKILL and marks the result
    /// timed out; the kill is a confirmed exit.
    #[test]
    fn the_deadline_kills_the_group_and_marks_timed_out() {
        let (mut child, pgid, _guard) = sleeping("echo ready; sleep 60");
        wait_ready(&mut child);
        let core = Core::new();
        held(&core, pgid);
        let observed = observe(
            &mut child,
            pgid,
            &core,
            Duration::from_millis(50),
            Duration::from_secs(5),
            Duration::from_millis(10),
        );
        assert!(observed.timed_out);
        assert!(!observed.unconfirmed);
        assert!(observed.exit.is_none(), "a signal death has no code");
    }

    /// A clean exit before the deadline is not reported as a timeout.
    #[test]
    fn a_clean_exit_is_not_a_timeout() {
        let (mut child, pgid, _guard) = sleeping("echo ready; sleep 0.05");
        wait_ready(&mut child);
        let core = Core::new();
        held(&core, pgid);
        let observed = observe(
            &mut child,
            pgid,
            &core,
            Duration::from_secs(30),
            Duration::from_secs(5),
            Duration::from_millis(10),
        );
        assert!(!observed.timed_out);
        assert!(!observed.unconfirmed);
        assert_eq!(observed.exit, Some(0));
        assert!(core.state.lock().unwrap().query_pgid.is_none());
    }

    #[test]
    fn cancellation_requested_before_pgid_registration_stops_the_child() {
        let (child, _pgid, _guard) = sleeping("echo ready; exec sleep 60");
        let mut child = child;
        wait_ready(&mut child);
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.query_running = true;
            st.query_stopping = true;
        }

        let observed = wait(child, &core);

        assert!(observed.cancelled);
        assert!(!observed.timed_out);
        assert!(!observed.unconfirmed);
        assert!(core.state.lock().unwrap().query_pgid.is_none());
    }

    #[test]
    fn stop_escalates_for_an_ignoring_child_without_becoming_a_timeout() {
        let (child, pgid, _guard) = sleeping("trap '' INT; echo ready; while :; do sleep 1; done");
        let mut child = child;
        wait_ready(&mut child);
        let core = Core::new();
        held(&core, pgid);
        core.state.lock().unwrap().query_stopping = true;
        let started = Instant::now();

        let observed = wait(child, &core);

        assert!(
            started.elapsed() >= STOP_GRACE,
            "SIGKILL escalation was skipped"
        );
        assert!(observed.cancelled);
        assert!(!observed.timed_out);
        assert!(!observed.unconfirmed);
        assert!(observed.exit.is_none());
        assert!(core.state.lock().unwrap().query_pgid.is_none());
    }

    /// A panic must release the slot: the next acquire may proceed.
    #[test]
    fn panic_releases_the_slot() {
        let core = Arc::new(Core::new());
        {
            let mut st = core.state.lock().unwrap();
            st.query_running = true;
            st.query_pgid = Some(i32::MAX);
            st.query_stopping = true;
        }
        let panicking = core.clone();
        let owner = thread::spawn(move || {
            let emit = || ();
            let _slot = QuerySlot {
                core: &panicking,
                emit,
            };
            panic!("injected worker panic");
        });
        assert!(owner.join().is_err());
        let st = core.state.lock().unwrap();
        assert!(!st.query_running);
        assert!(st.query_pgid.is_none());
        assert!(!st.query_stopping);
    }

    /// The wire shape the frontend's `QueryOutput` consumes.
    #[test]
    fn query_output_serializes_in_camel_case() {
        let json = serde_json::to_value(QueryOutput {
            exit_code: Some(2),
            stdout: "//a:b\n".to_string(),
            truncated: false,
            stderr_tail: "oops".to_string(),
            timed_out: false,
        })
        .unwrap();
        assert_eq!(json["exitCode"], 2);
        assert_eq!(json["stderrTail"], "oops");
        assert_eq!(json["timedOut"], false);
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
