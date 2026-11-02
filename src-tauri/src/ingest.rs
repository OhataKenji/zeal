//! Fast target-list ingestion and on-demand XML details for one target.
//!
//! Triggers: workspace open and manual refresh. Busy requests are rejected;
//! no pending query or post-run refresh is kept.

use crate::snapshot::{self, Status, Target};
use crate::state::{Core, IngestError};
use crate::xml::{self, ParsedQuery};
use rustix::event::{self, PollFd, PollFlags, Timespec};
use rustix::fs::{self, OFlags};
use rustix::process::Signal;
use std::io::{self, BufReader, Read};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Upper bound for one `bazel query`. First runs may include a bazelisk
/// download and a cold analysis cache.
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(300);

/// Allow a reaped child time to close both pipes before canceling readers.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Confirm a SIGKILL before giving the child to a detached reaper.
const KILL_CONFIRM: Duration = Duration::from_secs(2);

/// Bound the wait for a reader to acknowledge cancellation.
const READER_CONFIRM: Duration = Duration::from_millis(500);

const POLL: Duration = Duration::from_millis(50);
const STDERR_TAIL: usize = 8192;

/// A pipe reader owns its FD and can be stopped without waiting for EOF.
struct CancelableRead<R> {
    pipe: R,
    cancel: Arc<AtomicBool>,
    eof: bool,
}

impl<R: Read + AsFd> CancelableRead<R> {
    fn new(pipe: R, cancel: Arc<AtomicBool>) -> io::Result<Self> {
        let flags = fs::fcntl_getfl(&pipe)?;
        fs::fcntl_setfl(&pipe, flags | OFlags::NONBLOCK)?;
        Ok(Self {
            pipe,
            cancel,
            eof: false,
        })
    }

    fn read_once(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.pipe.read(buf) {
            Ok(0) => {
                self.eof = true;
                Ok(Some(0))
            }
            Ok(count) => Ok(Some(count)),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.wait_readable()?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn wait_readable(&self) -> io::Result<()> {
        let mut fds = [PollFd::new(&self.pipe, PollFlags::IN)];
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: 50_000_000,
        };
        match event::poll(&mut fds, Some(&timeout)) {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl<R: Read + AsFd> Read for CancelableRead<R> {
    #[expect(
        clippy::excessive_nesting,
        reason = "the retry loop must check cancellation before each read"
    )]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Ok(0);
            }
            if let Some(count) = self.read_once(buf)? {
                return Ok(count);
            }
        }
    }
}

/// Keep only the last stderr bytes while draining the pipe to EOF or cancel.
fn capture_stderr(reader: &mut CancelableRead<ChildStderr>) -> io::Result<Vec<u8>> {
    let mut tail = Vec::new();
    let mut buf = [0; 4096];
    loop {
        let count = reader.read(&mut buf)?;
        if count == 0 {
            return Ok(tail);
        }
        tail.extend_from_slice(&buf[..count]);
        let excess = tail.len().saturating_sub(STDERR_TAIL);
        tail.drain(..excess);
    }
}

/// Parsing and error capture run concurrently while the child is supervised.
struct QueryReaders<T> {
    stdout: JoinHandle<(Result<T, String>, bool)>,
    stderr: JoinHandle<(io::Result<Vec<u8>>, bool)>,
    cancel: Arc<AtomicBool>,
}

/// A missing worker result means its FD could not be confirmed reclaimed.
struct ReaderOutput<T> {
    stdout: Option<(Result<T, String>, bool)>,
    stderr: Option<(io::Result<Vec<u8>>, bool)>,
}

impl<T: Send + 'static> QueryReaders<T> {
    fn start(
        child: &mut Child,
        parse: impl FnOnce(&mut BufReader<CancelableRead<ChildStdout>>) -> Result<T, String>
        + Send
        + 'static,
    ) -> io::Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let stdout_pipe = child.stdout.take().expect("stdout piped");
        let stderr_pipe = child.stderr.take().expect("stderr piped");
        let stdout = CancelableRead::new(stdout_pipe, cancel.clone())?;
        let stderr = CancelableRead::new(stderr_pipe, cancel.clone())?;
        let stdout = thread::Builder::new()
            .name("query-stdout".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let parsed = parse(&mut reader);
                (parsed, reader.get_ref().eof)
            })?;
        let stderr = match thread::Builder::new()
            .name("query-stderr".into())
            .spawn(move || {
                let mut reader = stderr;
                let captured = capture_stderr(&mut reader);
                (captured, reader.eof)
            }) {
            Ok(stderr) => stderr,
            Err(error) => {
                let reclaimed = stop_after_spawn_failure(&cancel, stdout);
                return Err(spawn_failure_error(error, reclaimed));
            }
        };
        Ok(Self {
            stdout,
            stderr,
            cancel,
        })
    }

    fn finish(self, drain_deadline: Instant) -> ReaderOutput<T> {
        self.wait_until(drain_deadline);
        if !(self.stdout.is_finished() && self.stderr.is_finished()) {
            self.cancel.store(true, Ordering::Release);
            self.wait_until(Instant::now() + READER_CONFIRM);
        }
        let stdout = if self.stdout.is_finished() {
            Some(
                self.stdout
                    .join()
                    .unwrap_or_else(|_| (Err("query output parser failed".into()), false)),
            )
        } else {
            None
        };
        let stderr = if self.stderr.is_finished() {
            Some(
                self.stderr
                    .join()
                    .unwrap_or((Err(io::Error::other("query stderr reader failed")), false)),
            )
        } else {
            None
        };
        ReaderOutput { stdout, stderr }
    }

    fn wait_until(&self, deadline: Instant) {
        while !(self.stdout.is_finished() && self.stderr.is_finished()) && Instant::now() < deadline
        {
            thread::sleep(POLL);
        }
    }
}

fn stop_after_spawn_failure<T>(cancel: &AtomicBool, stdout: JoinHandle<T>) -> bool {
    cancel.store(true, Ordering::Release);
    let until = Instant::now() + READER_CONFIRM;
    while !stdout.is_finished() && Instant::now() < until {
        thread::sleep(POLL);
    }
    if stdout.is_finished() {
        let _ = stdout.join();
        true
    } else {
        log::error!("query stdout reader cleanup unconfirmed after spawn failure");
        false
    }
}

fn spawn_failure_error(error: io::Error, reclaimed: bool) -> io::Error {
    let note = if reclaimed {
        ""
    } else {
        "; stdout reader cleanup unconfirmed"
    };
    io::Error::other(format!("failed to start stderr reader: {error}{note}"))
}

/// `status` confirms direct-child reaping; otherwise a reaper owns the Child.
struct ChildExit {
    status: Option<ExitStatus>,
    error: Option<String>,
}

fn observe_child(child: Child, timeout: Duration) -> ChildExit {
    observe_child_with(child, timeout, Child::try_wait)
}

fn observe_child_with(
    child: Child,
    timeout: Duration,
    mut try_wait: impl FnMut(&mut Child) -> io::Result<Option<ExitStatus>>,
) -> ChildExit {
    let mut child = child;
    let pgid = child.id() as i32;
    let deadline = Instant::now() + timeout;
    let mut kill_deadline = None;
    loop {
        match try_wait(&mut child) {
            Ok(Some(status)) => {
                let error = kill_deadline.map(|_| format!("query timed out after {timeout:?}"));
                return ChildExit {
                    status: Some(status),
                    error,
                };
            }
            Ok(None) => (),
            Err(error) => {
                let summary = format!("failed to wait on query: {error}; exit unconfirmed");
                reap_later(child);
                return ChildExit {
                    status: None,
                    error: Some(summary),
                };
            }
        }
        let now = Instant::now();
        if kill_deadline.is_none() && now >= deadline {
            kill_query_group(pgid);
            kill_deadline = Some(now + KILL_CONFIRM);
        }
        if kill_deadline.is_some_and(|until| now >= until) {
            reap_later(child);
            return ChildExit {
                status: None,
                error: Some(format!(
                    "query timed out after {timeout:?}; direct-child exit unconfirmed"
                )),
            };
        }
        thread::sleep(POLL);
    }
}

fn kill_query_group(pgid: i32) {
    if let Err(error) = crate::runner::signal_group(pgid, Signal::KILL) {
        log::warn!("failed to kill query group: {error}");
    }
}

/// The reaper alone owns the unconfirmed Child; no later group signal is safe.
fn reap_later(mut child: Child) {
    thread::spawn(move || {
        if let Err(error) = child.wait() {
            log::error!("query child reaping failed: {error}");
        }
    });
}

fn finish_query<T>(
    exit: ChildExit,
    readers: ReaderOutput<T>,
    has_data: impl Fn(&T) -> bool,
) -> Outcome<T> {
    let code = exit.status.and_then(|status| status.code());
    let mut errors = output_errors(&readers);
    if let Some(error) = exit.error {
        errors.insert(0, error);
    }
    if !errors.is_empty() {
        return Outcome::Failed(IngestError {
            exit_code: code,
            summary: errors.join("; "),
        });
    }
    let data = readers
        .stdout
        .expect("complete stdout reader returned a result")
        .0
        .expect("complete stdout reader parsed data");
    if code == Some(0) {
        Outcome::Ok(data)
    } else {
        let stderr = readers
            .stderr
            .expect("complete stderr reader returned a result")
            .0
            .expect("complete stderr reader captured");
        let stderr_tail = String::from_utf8_lossy(&stderr);
        let error = IngestError {
            exit_code: code,
            summary: summarize(&stderr_tail),
        };
        if has_data(&data) {
            Outcome::Partial(data, error)
        } else {
            Outcome::Failed(error)
        }
    }
}

fn output_errors<T>(readers: &ReaderOutput<T>) -> Vec<String> {
    let mut errors = Vec::new();
    match &readers.stdout {
        None => {
            let error = "could not reclaim stdout reader";
            log::error!("{error}");
            errors.push(error.into());
        }
        Some((_, false)) => errors.push("query stdout incomplete".into()),
        _ => (),
    }
    match &readers.stderr {
        None => {
            let error = "could not reclaim stderr reader";
            log::error!("{error}");
            errors.push(error.into());
        }
        Some((_, false)) => errors.push("query stderr incomplete".into()),
        _ => (),
    }
    if let Some((Err(error), _)) = &readers.stdout {
        errors.push(error.clone());
    }
    if let Some((Err(error), _)) = &readers.stderr {
        errors.push(format!("could not read query stderr: {error}"));
    }
    errors
}

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
        let outcome = fetch(
            &ws,
            &bazel,
            &query,
            "label_kind",
            |input| snapshot::parse_label_kind(input),
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
    let (ws, bazel, query) = acquire_detail(&core, label, fetched_at)?;
    let _slot = DetailSlot {
        app: app.clone(),
        core: core.clone(),
    };
    core.emit(&app);
    let (query, status) = match fetch(
        &ws,
        &bazel,
        &query,
        "xml",
        |output| Ok(xml::parse_query(output)),
        |query: &ParsedQuery| !query.rules.is_empty(),
    ) {
        Outcome::Ok(query) => (query, Status::Ok),
        Outcome::Partial(query, _) => (query, Status::Partial),
        Outcome::Failed(error) => return Err(error.summary),
    };
    snapshot::select_detail(snapshot::build(&query, status), label)
        .ok_or_else(|| "target not found in the current Scope".to_string())
}

fn acquire_detail(
    core: &Core,
    label: &str,
    fetched_at: u64,
) -> Result<(PathBuf, PathBuf, String), String> {
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
    let query = crate::scope::target_query(label)?;
    st.detail_query_running = true;
    Ok((ws, bazel, query))
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

/// Run one scoped query and parse stdout as it arrives.
#[tracing::instrument(target = "zeal::operations", name = "bazel.query", skip_all)]
fn fetch<T: Send + 'static>(
    ws: &Path,
    bazel: &Path,
    query: &str,
    output: &str,
    parse: impl FnOnce(&mut BufReader<CancelableRead<ChildStdout>>) -> Result<T, String>
    + Send
    + 'static,
    has_data: impl Fn(&T) -> bool,
) -> Outcome<T> {
    fetch_with_timeout(ws, bazel, query, output, parse, has_data, QUERY_TIMEOUT)
}

fn fetch_with_timeout<T: Send + 'static>(
    ws: &Path,
    bazel: &Path,
    query: &str,
    output: &str,
    parse: impl FnOnce(&mut BufReader<CancelableRead<ChildStdout>>) -> Result<T, String>
    + Send
    + 'static,
    has_data: impl Fn(&T) -> bool,
    timeout: Duration,
) -> Outcome<T> {
    let mut child = match Command::new(bazel)
        .arg("query")
        .arg(query)
        .arg(format!("--output={output}"))
        .args(["--noimplicit_deps", "--keep_going"])
        .current_dir(ws)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
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
    let readers = match QueryReaders::start(&mut child, parse) {
        Ok(readers) => readers,
        Err(error) => {
            let exit = observe_child(child, Duration::ZERO);
            return Outcome::Failed(IngestError {
                exit_code: exit.status.and_then(|status| status.code()),
                summary: format!(
                    "failed to start query readers: {error}{}",
                    if exit.status.is_none() {
                        "; direct-child cleanup unconfirmed"
                    } else {
                        ""
                    }
                ),
            });
        }
    };
    let exit = observe_child(child, timeout);
    let output = readers.finish(Instant::now() + DRAIN_GRACE);
    finish_query(exit, output, has_data)
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
    use crate::snapshot::{Meta, TargetIndex};
    use std::fs;

    struct QueryFixture(PathBuf);

    impl QueryFixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("zeal-ingest-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn script(&self, body: &str) {
            fs::write(self.0.join("query"), body).unwrap();
        }

        fn label_kind(&self, timeout: Duration) -> Outcome<Vec<Target>> {
            fetch_with_timeout(
                &self.0,
                Path::new("/bin/sh"),
                "//...",
                "label_kind",
                |input| snapshot::parse_label_kind(input),
                |targets: &Vec<Target>| !targets.is_empty(),
                timeout,
            )
        }

        fn detail(&self, timeout: Duration) -> Outcome<ParsedQuery> {
            fetch_with_timeout(
                &self.0,
                Path::new("/bin/sh"),
                "//app:lib",
                "xml",
                |output| Ok(xml::parse_query(output)),
                |query: &ParsedQuery| !query.rules.is_empty(),
                timeout,
            )
        }
    }

    impl Drop for QueryFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn assert_incomplete_pipe(fixture: &QueryFixture, redirected: &str) {
        // The direct shell exits. Its background child keeps one pipe
        // open longer than DRAIN_GRACE, then exits on its own.
        fixture.script(&format!(
            "sleep 4 {redirected} & printf 'cc_library rule //app:lib\\n'; exit 0\n"
        ));
        let start = Instant::now();
        let Outcome::Failed(error) = fixture.label_kind(Duration::from_secs(2)) else {
            panic!("incomplete pipe must be reported");
        };
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(error.summary.contains("incomplete"), "{}", error.summary);
        assert!(
            !error.summary.contains("could not reclaim"),
            "readers must join and close their FDs: {}",
            error.summary
        );
    }

    #[test]
    fn fetch_parses_target_list_and_selected_target_xml() {
        let fixture = QueryFixture::new();
        fixture.script("printf 'cc_library rule //app:lib\\n'\n");
        let Outcome::Ok(targets) = fixture.label_kind(Duration::from_secs(2)) else {
            panic!("valid target list should complete");
        };
        assert_eq!(targets[0].label, "//app:lib");

        fixture.script("printf '<query><rule name=\"//app:lib\" class=\"cc_library\"/></query>'\n");
        let outcome = fixture.detail(Duration::from_secs(2));
        let Outcome::Ok(query) = outcome else {
            panic!("valid XML detail should complete");
        };
        assert_eq!(query.rules[0].label, "//app:lib");
    }

    #[test]
    fn fetch_keeps_partial_targets_and_stderr_diagnostic() {
        let fixture = QueryFixture::new();
        fixture.script("printf 'cc_library rule //app:lib\\n'; echo 'partial query' >&2; exit 7\n");
        let Outcome::Partial(targets, error) = fixture.label_kind(Duration::from_secs(2)) else {
            panic!("nonzero exit with targets should be partial");
        };
        assert_eq!(targets[0].label, "//app:lib");
        assert_eq!(error.exit_code, Some(7));
        assert_eq!(error.summary, "partial query");
    }

    #[test]
    fn descendant_held_pipes_are_canceled_and_reclaimed_repeatedly() {
        let fixture = QueryFixture::new();
        for redirected in ["2>/dev/null", "1>/dev/null", "2>/dev/null", "1>/dev/null"] {
            assert_incomplete_pipe(&fixture, redirected);
        }

        fixture.script("printf 'cc_library rule //app:recovered\\n'\n");
        let Outcome::Ok(targets) = fixture.label_kind(Duration::from_secs(2)) else {
            panic!("subsequent query should work after incomplete output");
        };
        assert_eq!(targets[0].label, "//app:recovered");
    }

    #[test]
    fn incomplete_xml_detail_fails_and_next_detail_succeeds() {
        let fixture = QueryFixture::new();
        fixture.script(
            "sleep 4 2>/dev/null & printf '<query><rule name=\"//app:lib\" class=\"cc_library\"/></query>'; exit 0\n",
        );
        let start = Instant::now();
        let Outcome::Failed(error) = fixture.detail(Duration::from_secs(2)) else {
            panic!("XML with a held stdout pipe must be incomplete");
        };
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(error.summary.contains("query stdout incomplete"));
        assert!(!error.summary.contains("could not reclaim"));

        fixture.script("printf '<query><rule name=\"//app:lib\" class=\"cc_library\"/></query>'\n");
        let Outcome::Ok(query) = fixture.detail(Duration::from_secs(2)) else {
            panic!("subsequent XML detail should complete");
        };
        assert_eq!(query.rules[0].label, "//app:lib");
    }

    #[test]
    fn timed_out_child_is_reaped_and_readers_stop() {
        let fixture = QueryFixture::new();
        fixture.script("exec sleep 30\n");
        let start = Instant::now();
        let Outcome::Failed(error) = fixture.label_kind(Duration::from_millis(100)) else {
            panic!("timed out query should fail");
        };
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(error.summary.contains("timed out"), "{}", error.summary);
        assert!(!error.summary.contains("exit unconfirmed"));
        assert!(!error.summary.contains("could not reclaim"));

        let child = Command::new("/bin/sh")
            .args(["-c", "exec sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        let exit = observe_child(child, Duration::from_millis(100));
        assert!(exit.status.is_some(), "direct child was not reaped");
    }

    #[test]
    fn wait_error_does_not_signal_an_unconfirmed_process_group() {
        let fixture = QueryFixture::new();
        let child = Command::new("/bin/sh")
            .args(["-c", "sleep 0.2; printf finished > marker"])
            .current_dir(&fixture.0)
            .process_group(0)
            .spawn()
            .unwrap();
        let exit = observe_child_with(child, Duration::from_millis(100), |_| {
            Err(io::Error::other("injected wait failure"))
        });
        assert!(exit.status.is_none());
        assert!(exit.error.unwrap().contains("injected wait failure"));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !fixture.0.join("marker").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            fixture.0.join("marker").exists(),
            "child was signaled after wait error"
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
        assert!(acquire_detail(&core, "//app:chosen", 6).is_err());
        assert!(acquire_detail(&core, "//app:unknown", 7).is_err());
        let (ws, bazel, query) = acquire_detail(&core, "//app:chosen", 7).unwrap();
        assert_eq!(ws, PathBuf::from("/ws"));
        assert_eq!(bazel, PathBuf::from("/bin/bazel"));
        assert_eq!(query, crate::scope::target_query("//app:chosen").unwrap());
        assert!(core.state.lock().unwrap().detail_query_running);
        assert!(acquire_detail(&core, "//app:chosen", 7).is_err());
    }
}
