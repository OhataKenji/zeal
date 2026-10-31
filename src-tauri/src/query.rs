//! Ad-hoc `bazel query` for the query editor.
//!
//! Distinct from ingest's fixed `bazel query //...` snapshot fetch: the
//! expression and the output format come from the editor. Execution holds the
//! same serialized bazel slot as ingest and run, stops the same way (SIGINT,
//! then SIGKILL after a grace), and keeps bounded output: the first bytes of
//! stdout and the tail of stderr.
//!
//! Stop is best-effort: a killed group whose exit cannot be confirmed still
//! releases the slot, marked stopped with no exit code.

use crate::ingest::QUERY_TIMEOUT;
use crate::runner::signal_group;
use crate::state::{Core, CoreState};
use rustix::process::Signal;
use serde::Serialize;
use std::io::Read;
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

/// Time SIGINT gets to end the query before its group is killed.
const STOP_INT_GRACE: Duration = Duration::from_secs(3);

/// Time SIGKILL gets to end the query before its exit is called unconfirmed.
const STOP_CONFIRM: Duration = Duration::from_secs(2);

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
    /// A stop was requested or the exit could not be confirmed.
    pub stopped: bool,
}

/// Arguments after the bazel executable. The expression is one `Command::arg`:
/// nothing is split, quoted or escaped. No `--tool_tag` (query versions may
/// reject it) and no `--keep_going` (partial results would be unusable).
fn argv(expr: &str, output: &str, no_implicit: bool, no_tool: bool) -> Vec<String> {
    let mut args = vec![
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

/// Run one ad-hoc `bazel query` in the open workspace. Busy requests — a run,
/// an ingest or another query in flight — are rejected rather than queued.
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
    let (ws, bazel, generation) = acquire(&core)?;
    // Constructed before anything else can panic: unwinding releases the slot.
    let _slot = QuerySlot {
        core: &core,
        generation,
        emit: || core.emit(&app),
    };
    core.emit(&app);
    // Its own process group: a stop reaches bazel and whatever it started.
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
    let pgid = child.id() as i32;
    {
        let mut st = core.state.lock().expect("application state lock poisoned");
        if st.query_gen == generation {
            st.query_pgid = Some(pgid);
        }
    }
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
    let observed = observe(
        &mut child,
        pgid,
        &core,
        QUERY_TIMEOUT,
        STOP_INT_GRACE,
        STOP_CONFIRM,
        POLL,
    );
    // An unconfirmed exit means the child's fate is unknown: spawn a detached
    // reaper so it cannot linger as a zombie after the slot is released.
    if observed.unconfirmed {
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
    // A starved reader must not fake an empty result: dropped stdout is
    // reported as truncated and a missing stderr capture gets the same note
    // an unconfirmed exit does.
    let (stdout, truncated) = match drain(&out_rx, observed.unconfirmed) {
        Some((bytes, dropped)) => (bytes, dropped),
        None => (Vec::new(), true),
    };
    let stderr = drain(&err_rx, observed.unconfirmed);
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
        stopped: observed.stopped,
    })
}

/// Take the serialized bazel slot for this query: reject while a run, an
/// ingest or another query holds it, and require an open workspace with a
/// resolved bazel. The generation stamps the hold so a stale worker cannot
/// release a successor's slot.
fn acquire(core: &Core) -> Result<(PathBuf, PathBuf, u64), String> {
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
    st.query_stop_started = None;
    st.query_gen += 1;
    Ok((ws, bazel, st.query_gen))
}

/// Stop the ad-hoc query in flight: SIGINT to its process group; the worker
/// escalates to SIGKILL after the grace. Does nothing when no query is held
/// or a stop is already accepted, so the slot is never held by a stop.
pub fn stop(app: AppHandle, core: Arc<Core>) -> Result<(), String> {
    begin_stop(&mut core.state.lock().expect("application state lock poisoned"));
    core.emit(&app);
    Ok(())
}

/// The first accepted stop flags the query, fixes the escalation start and
/// SIGINTs the group. Only the worker's wait loop can escalate or reap. The
/// flag is set even before the pgid lands (acquire → spawn window): `observe`
/// delivers the INT on its first poll, so the request is never lost.
/// Shared with `runner::exit_stop` so app exit applies the same request once.
pub(crate) fn begin_stop(st: &mut CoreState) {
    if !st.query_running || st.query_stopping {
        return;
    }
    st.query_stopping = true;
    st.query_stop_started = Some(Instant::now());
    if let Some(pgid) = st.query_pgid
        && let Err(e) = signal_group(pgid, Signal::INT)
    {
        log::warn!("SIGINT to query group failed: {e}");
    }
}

/// How supervising the query child ended.
struct Observed {
    /// Confirmed exit code (`None` for a signal death), if one was observed.
    exit: Option<i32>,
    /// The deadline passed; the group was killed.
    timed_out: bool,
    /// A stop was requested, or the exit could not be confirmed.
    stopped: bool,
    /// `wait` failed or the group outlived SIGKILL's confirm window.
    unconfirmed: bool,
}

/// Poll the child and apply the stop/timeout escalation under the same lock.
/// Once try_wait reaps the child, its process group is revoked immediately:
/// the pid is no longer ours to signal.
#[tracing::instrument(target = "zeal::operations", name = "query.wait", skip_all)]
fn observe(
    child: &mut Child,
    pgid: i32,
    core: &Core,
    deadline: Duration,
    stop_grace: Duration,
    kill_confirm: Duration,
    poll: Duration,
) -> Observed {
    let end = Instant::now() + deadline;
    let mut sigint_at: Option<Instant> = None;
    let mut killed_at: Option<Instant> = None;
    let mut timed_out = false;
    loop {
        let mut st = core.state.lock().expect("application state lock poisoned");
        match child.try_wait() {
            Ok(Some(status)) => {
                if st.query_pgid == Some(pgid) {
                    st.query_pgid = None;
                }
                return Observed {
                    exit: status.code(),
                    timed_out,
                    // A query that finished cleanly despite an accepted stop
                    // is a normal success, not a stopped one.
                    stopped: st.query_stopping && status.code() != Some(0),
                    unconfirmed: false,
                };
            }
            Ok(None) => (),
            Err(_) => {
                // A failed wait can mean another reaper consumed the child;
                // its former pid is no longer safe to signal.
                if st.query_pgid == Some(pgid) {
                    st.query_pgid = None;
                }
                return Observed {
                    exit: None,
                    timed_out,
                    stopped: true,
                    unconfirmed: true,
                };
            }
        }
        let now = Instant::now();
        if st.query_stopping && sigint_at.is_none() {
            // The requester signals too; this second INT is harmless and
            // covers a stop flag left without a delivered signal.
            if let Err(e) = signal_group(pgid, Signal::INT) {
                log::warn!("SIGINT to query group failed: {e}");
            }
            sigint_at = Some(st.query_stop_started.unwrap_or(now));
        }
        if now >= end {
            timed_out = true;
        }
        let kill_due = timed_out || sigint_at.is_some_and(|at| at.elapsed() >= stop_grace);
        if kill_due && killed_at.is_none() {
            if let Err(e) = signal_group(pgid, Signal::KILL) {
                log::warn!("SIGKILL to query group failed: {e}");
            }
            killed_at = Some(now);
        }
        if let Some(killed) = killed_at
            && killed.elapsed() >= kill_confirm
        {
            if st.query_pgid == Some(pgid) {
                st.query_pgid = None;
            }
            return Observed {
                exit: None,
                timed_out,
                stopped: true,
                unconfirmed: true,
            };
        }
        drop(st);
        thread::sleep(poll);
    }
}

/// A panic in the worker must release the query slot. A successor's hold —
/// marked by a newer generation — is never touched.
struct QuerySlot<'a, F: Fn()> {
    core: &'a Core,
    generation: u64,
    emit: F,
}

impl<F: Fn()> Drop for QuerySlot<'_, F> {
    fn drop(&mut self) {
        let mut st = self
            .core
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if st.query_gen == self.generation {
            clear_query(&mut st);
        }
        drop(st);
        // Avoid a second panic from an event emission while unwinding.
        if !thread::panicking() {
            (self.emit)();
        }
    }
}

/// Clear the query slot: nothing is in flight, nothing is ours to signal.
fn clear_query(st: &mut CoreState) {
    st.query_running = false;
    st.query_pgid = None;
    st.query_stopping = false;
    st.query_stop_started = None;
}

/// Keep the first `cap` bytes of the stream and drain the rest: the child
/// must never block on a full pipe. Returns the bytes and whether any were
/// dropped.
fn capture_head(pipe: &mut impl Read, cap: usize) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let kept = n.min(cap - buf.len());
                buf.extend_from_slice(&chunk[..kept]);
                truncated |= kept < n;
            }
        }
    }
    (buf, truncated)
}

/// Keep the last `cap` bytes of the stream.
fn capture_tail(pipe: &mut impl Read, cap: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > cap {
                    buf.drain(..buf.len() - cap);
                }
            }
        }
    }
    buf
}

/// Receive a reader's capture. Even a confirmed exit is bounded: a descendant
/// still holding the pipe would otherwise wedge the bazel slot forever — the
/// same hazard runner bounds with its drain grace. The confirmed wait is the
/// longer `DRAIN_GRACE` so ordinary end-of-stream delivery is not raced.
/// `None` means the reader never delivered within the bound; callers must
/// surface that loss rather than report a fabricated empty capture.
fn drain<T>(rx: &mpsc::Receiver<T>, unconfirmed: bool) -> Option<T> {
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

    /// The first stop request, the way `stop` applies it.
    fn request_stop(core: &Core) {
        begin_stop(&mut core.state.lock().unwrap());
    }

    #[test]
    fn argv_has_the_expression_the_output_and_the_flag_switches() {
        assert_eq!(
            argv("deps(//a:b)", "label", false, false),
            ["query", "deps(//a:b)", "--output=label"].map(String::from)
        );
        assert_eq!(
            argv("//...", "graph", false, false),
            ["query", "//...", "--output=graph", "--nograph:factored"].map(String::from)
        );
        assert_eq!(
            argv("//a:b", "label", true, false),
            ["query", "//a:b", "--output=label", "--noimplicit_deps"].map(String::from)
        );
        assert_eq!(
            argv("//a:b", "label", false, true),
            ["query", "//a:b", "--output=label", "--notool_deps"].map(String::from)
        );
        assert_eq!(
            argv("//a:b", "graph", true, true),
            [
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
    fn validation_rejects_unsafe_or_unsupported_input() {
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
        for expr in [
            "deps(//a:b)",
            "//...",
            "a\tb",
            "deps(//a)\nunion\ndeps(//b)",
            "a\rb",
        ] {
            assert!(validate_expr(expr).is_ok(), "{expr:?}");
        }
        let long = "a".repeat(MAX_EXPR_LEN + 1);
        assert!(validate_expr(&long).is_err());
        assert!(validate_expr(&"a".repeat(MAX_EXPR_LEN)).is_ok());
        for output in ["xml", "", "Label"] {
            assert!(validate_output(output).is_err(), "{output:?}");
        }
        for output in ["label", "graph"] {
            assert!(validate_output(output).is_ok(), "{output:?}");
        }
        // Error strings name the rule, never the expression.
        let msg = validate_expr("-deps(//secret:t)").unwrap_err();
        assert!(!msg.contains("secret"), "{msg}");
        let msg = validate_expr("bad\0expr").unwrap_err();
        assert!(!msg.contains("bad"), "{msg}");
    }

    /// Each way the slot can be held rejects a new query; a held query rejects
    /// a second one. The fields `run` leaves behind are all set at acquire.
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
            |st| st.run = Some("r".into()),
            |st| st.query_running = true,
        ];
        for hold in holds {
            hold(&mut core.state.lock().unwrap());
            assert!(acquire(&core).is_err());
            let mut st = core.state.lock().unwrap();
            st.ingest_running = false;
            st.run = None;
            st.query_running = false;
        }

        let (ws, bazel, generation) = acquire(&core).unwrap();
        assert_eq!(ws, PathBuf::from("/ws"));
        assert_eq!(bazel, PathBuf::from("/bin/bazel"));
        let st = core.state.lock().unwrap();
        assert!(st.query_running);
        assert!(st.query_pgid.is_none());
        assert!(!st.query_stopping);
        assert_eq!(st.query_gen, generation);
        drop(st);
        assert!(acquire(&core).is_err(), "a held query rejects the next");
    }

    /// SIGINT ends a group that honours it; no KILL is needed and the exit is
    /// confirmed.
    #[test]
    fn a_stop_signals_the_group_and_observes_the_exit() {
        let (mut child, pgid, _guard) = sleeping("echo ready; sleep 60");
        wait_ready(&mut child);
        let core = Core::new();
        held(&core, pgid);
        request_stop(&core);
        let observed = observe(
            &mut child,
            pgid,
            &core,
            Duration::from_secs(30),
            STOP_INT_GRACE,
            STOP_CONFIRM,
            Duration::from_millis(10),
        );
        assert!(observed.stopped);
        assert!(!observed.timed_out);
        assert!(!observed.unconfirmed);
        assert!(core.state.lock().unwrap().query_pgid.is_none());
    }

    /// A group that ignores SIGINT is killed after the grace; its exit by
    /// signal is still a confirmed exit.
    #[test]
    fn an_ignoring_group_is_killed_after_the_grace() {
        let (mut child, pgid, _guard) = sleeping("trap '' INT; echo ready; sleep 60");
        wait_ready(&mut child);
        let core = Core::new();
        held(&core, pgid);
        request_stop(&core);
        let grace = Duration::from_millis(50);
        let started = Instant::now();
        let observed = observe(
            &mut child,
            pgid,
            &core,
            Duration::from_secs(30),
            grace,
            Duration::from_secs(5),
            Duration::from_millis(10),
        );
        assert!(started.elapsed() >= grace, "the grace was skipped");
        assert!(observed.stopped);
        assert!(!observed.unconfirmed, "the kill itself is confirmed");
        assert!(observed.exit.is_none(), "a signal death has no code");
        assert!(core.state.lock().unwrap().query_pgid.is_none());
    }

    /// The deadline sends the group straight to SIGKILL and marks the result
    /// timed out; it is not a requested stop.
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
            STOP_INT_GRACE,
            Duration::from_secs(5),
            Duration::from_millis(10),
        );
        assert!(observed.timed_out);
        assert!(!observed.stopped, "a timeout is not a requested stop");
        assert!(!observed.unconfirmed);
        assert!(observed.exit.is_none(), "a signal death has no code");
    }

    /// A panic must release the slot; a stale worker must not clear the
    /// generation a successor took.
    #[test]
    fn panic_releases_the_slot_but_never_a_successors() {
        let core = Arc::new(Core::new());
        {
            let mut st = core.state.lock().unwrap();
            st.query_running = true;
            st.query_pgid = Some(i32::MAX);
            st.query_stopping = true;
            st.query_gen = 7;
        }
        let panicking = core.clone();
        let owner = thread::spawn(move || {
            let _slot = QuerySlot {
                core: &panicking,
                generation: 7,
                emit: || {},
            };
            panic!("injected worker panic");
        });
        assert!(owner.join().is_err());
        {
            let st = core.state.lock().unwrap();
            assert!(!st.query_running);
            assert!(st.query_pgid.is_none());
            assert!(!st.query_stopping);
            assert!(st.query_stop_started.is_none());
        }
        // A successor took the freed slot; the stale worker's guard is a no-op.
        {
            let mut st = core.state.lock().unwrap();
            st.query_running = true;
            st.query_pgid = Some(42);
            st.query_gen = 8;
        }
        let stale = core.clone();
        let owner = thread::spawn(move || {
            let _slot = QuerySlot {
                core: &stale,
                generation: 7,
                emit: || {},
            };
            panic!("stale worker panic");
        });
        assert!(owner.join().is_err());
        let st = core.state.lock().unwrap();
        assert!(st.query_running, "the successor's slot stands");
        assert_eq!(st.query_pgid, Some(42));
    }

    /// A stop requested before the pgid lands still flags the query: the
    /// worker's wait loop delivers the INT itself.
    #[test]
    fn a_stop_before_the_pgid_lands_still_flags_the_query() {
        let core = Core::new();
        core.state.lock().unwrap().query_running = true;
        request_stop(&core);
        let st = core.state.lock().unwrap();
        assert!(st.query_stopping);
        assert!(st.query_stop_started.is_some());
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
            stopped: false,
        })
        .unwrap();
        assert_eq!(json["exitCode"], 2);
        assert_eq!(json["stderrTail"], "oops");
        assert_eq!(json["timedOut"], false);
        assert_eq!(json["stopped"], false);
        assert!(json.get("stderr_tail").is_none());
    }

    #[test]
    fn captures_keep_the_stdout_head_and_the_stderr_tail() {
        let data = "0123456789".repeat(2000); // 20_000 bytes
        let (head, truncated) = capture_head(&mut data.as_bytes(), 4096);
        assert_eq!(head, data.as_bytes()[..4096]);
        assert!(truncated);

        let tail = capture_tail(&mut data.as_bytes(), 100);
        assert_eq!(tail, data.as_bytes()[data.len() - 100..]);

        let small = "short";
        let (head, truncated) = capture_head(&mut small.as_bytes(), 4096);
        assert_eq!(head, b"short");
        assert!(!truncated);
    }
}
