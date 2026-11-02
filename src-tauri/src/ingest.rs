//! Fast target-list ingestion and on-demand XML details for one target.
//!
//! Triggers: workspace open and manual refresh. Busy requests are rejected;
//! no pending query or post-run refresh is kept.

use crate::snapshot::{self, Status, Target};
use crate::state::{Core, IngestError};
use crate::xml::{self, ParsedQuery};
use rustix::process::Signal;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tempfile::NamedTempFile;

/// Upper bound for one `bazel query`. First runs may include a bazelisk
/// download and a cold analysis cache.
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(300);

/// Confirm a SIGKILL before giving the child to a detached reaper.
const KILL_CONFIRM: Duration = Duration::from_secs(2);

const POLL: Duration = Duration::from_millis(50);
const STDERR_TAIL: usize = 8192;
/// Maximum captured bytes accepted for either stream after the direct child exits.
const MAX_OUTPUT_BYTES: u64 = 512 * 1024 * 1024;

/// The returned reader has an independent file offset from the child's writer.
fn capture_file() -> io::Result<(Stdio, File)> {
    let named = NamedTempFile::new()?;
    let reader = named.reopen()?;
    Ok((Stdio::from(named.into_file()), reader))
}

fn output_len(file: &File, stream: &str) -> Result<u64, String> {
    let len = file
        .metadata()
        .map_err(|error| format!("could not stat query {stream}: {error}"))?
        .len();
    if len > MAX_OUTPUT_BYTES {
        return Err(format!("query {stream} exceeds 512 MiB"));
    }
    Ok(len)
}

fn read_stderr_tail(mut stderr: File, len: u64) -> io::Result<Vec<u8>> {
    let tail_len = len.min(STDERR_TAIL as u64);
    stderr.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0; tail_len as usize];
    stderr.read_exact(&mut tail)?;
    Ok(tail)
}

/// Record read errors even when an XML parser treats them as end of input.
struct CheckedRead {
    inner: io::Take<File>,
    error: Option<String>,
}

impl Read for CheckedRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.inner.read(buf) {
            Ok(0) if !buf.is_empty() && self.inner.limit() > 0 => {
                self.error = Some("query stdout shortened after child exit".into());
                Ok(0)
            }
            Err(error) => {
                self.error = Some(error.to_string());
                Err(error)
            }
            other => other,
        }
    }
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
    status: ExitStatus,
    data: T,
    stderr_tail: &[u8],
    has_data: impl Fn(&T) -> bool,
) -> Outcome<T> {
    if status.success() {
        Outcome::Ok(data)
    } else {
        let stderr_tail = String::from_utf8_lossy(stderr_tail);
        let error = IngestError {
            exit_code: status.code(),
            summary: summarize(&stderr_tail),
        };
        if has_data(&data) {
            Outcome::Partial(data, error)
        } else {
            Outcome::Failed(error)
        }
    }
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

/// Run one scoped query and parse only bytes present when its direct child exits.
#[tracing::instrument(target = "zeal::operations", name = "bazel.query", skip_all)]
fn fetch<T>(
    ws: &Path,
    bazel: &Path,
    query: &str,
    output: &str,
    parse: impl FnOnce(&mut dyn BufRead) -> Result<T, String>,
    has_data: impl Fn(&T) -> bool,
) -> Outcome<T> {
    fetch_with_timeout(ws, bazel, query, output, parse, has_data, QUERY_TIMEOUT)
}

fn fetch_with_timeout<T>(
    ws: &Path,
    bazel: &Path,
    query: &str,
    output: &str,
    parse: impl FnOnce(&mut dyn BufRead) -> Result<T, String>,
    has_data: impl Fn(&T) -> bool,
    timeout: Duration,
) -> Outcome<T> {
    let (stdout_writer, stdout_reader) = match capture_file() {
        Ok(files) => files,
        Err(error) => return failed(None, format!("failed to create stdout capture: {error}")),
    };
    let (stderr_writer, stderr_reader) = match capture_file() {
        Ok(files) => files,
        Err(error) => return failed(None, format!("failed to create stderr capture: {error}")),
    };
    let child = match Command::new(bazel)
        .arg("query")
        .arg(query)
        .arg(format!("--output={output}"))
        .args(["--noimplicit_deps", "--keep_going"])
        .current_dir(ws)
        .stdin(Stdio::null())
        .stdout(stdout_writer)
        .stderr(stderr_writer)
        .process_group(0)
        .spawn()
    {
        Ok(c) => c,
        Err(error) => {
            return failed(
                None,
                format!("failed to spawn {}: {error}", bazel.display()),
            );
        }
    };
    let exit = observe_child(child, timeout);
    if let Some(error) = exit.error {
        return failed(exit.status.and_then(|status| status.code()), error);
    }
    let Some(status) = exit.status else {
        return failed(None, "query exit unconfirmed".into());
    };
    // Freeze both lengths before reading either file. Descendant writes later
    // are outside this query's result.
    let stdout_len = match output_len(&stdout_reader, "stdout") {
        Ok(len) => len,
        Err(error) => return failed(status.code(), error),
    };
    let stderr_len = match output_len(&stderr_reader, "stderr") {
        Ok(len) => len,
        Err(error) => return failed(status.code(), error),
    };
    let stderr_tail = match read_stderr_tail(stderr_reader, stderr_len) {
        Ok(tail) => tail,
        Err(error) => {
            return failed(
                status.code(),
                format!("could not read query stderr: {error}"),
            );
        }
    };
    let mut stdout = BufReader::new(CheckedRead {
        inner: stdout_reader.take(stdout_len),
        error: None,
    });
    let data = match parse(&mut stdout) {
        Ok(data) => data,
        Err(error) => return failed(status.code(), error),
    };
    if let Some(error) = &stdout.get_ref().error {
        return failed(
            status.code(),
            format!("could not read query stdout: {error}"),
        );
    }
    finish_query(status, data, &stderr_tail, has_data)
}

fn failed<T>(exit_code: Option<i32>, summary: String) -> Outcome<T> {
    Outcome::Failed(IngestError { exit_code, summary })
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

    fn heavy_stderr_script(result: &str) -> String {
        format!(
            "i=0; while [ \"$i\" -lt 40 ]; do printf '%8192s' '' >&2; i=$((i + 1)); done; printf 'final diagnostic marker\\n' >&2; {result}\n"
        )
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
    fn stderr_tail_reads_only_the_last_eight_kib() {
        use std::io::Write;

        let mut file = NamedTempFile::new().unwrap();
        let mut output = vec![b' '; 320_000];
        output.extend_from_slice(b"\nfinal diagnostic marker\n");
        file.write_all(&output).unwrap();
        let reader = file.reopen().unwrap();
        let tail = read_stderr_tail(reader, output.len() as u64).unwrap();
        assert_eq!(tail.len(), STDERR_TAIL);
        assert_eq!(tail, output[output.len() - STDERR_TAIL..]);
        assert_eq!(
            summarize(&String::from_utf8_lossy(&tail)),
            "final diagnostic marker"
        );
    }

    #[test]
    fn heavy_stderr_preserves_partial_query() {
        let fixture = QueryFixture::new();
        fixture.script(&heavy_stderr_script(
            "printf 'cc_library rule //app:lib\\n'; exit 9",
        ));
        let Outcome::Partial(targets, error) = fixture.label_kind(Duration::from_secs(2)) else {
            panic!("large stderr and valid stdout should produce a partial result");
        };
        assert_eq!(targets[0].label, "//app:lib");
        assert_eq!(error.exit_code, Some(9));
        assert_eq!(error.summary, "final diagnostic marker");
    }

    #[test]
    fn heavy_stderr_preserves_partial_detail() {
        let fixture = QueryFixture::new();
        fixture.script(&heavy_stderr_script(
            "printf '<query><rule name=\"//app:lib\" class=\"cc_library\"/></query>'; exit 9",
        ));
        let Outcome::Partial(query, error) = fixture.detail(Duration::from_secs(2)) else {
            panic!("large stderr and valid XML should produce a partial result");
        };
        assert_eq!(query.rules[0].label, "//app:lib");
        assert_eq!(error.exit_code, Some(9));
        assert_eq!(error.summary, "final diagnostic marker");
    }

    #[test]
    fn heavy_stderr_reports_failed_detail() {
        let fixture = QueryFixture::new();
        fixture.script(&heavy_stderr_script("exit 7"));
        let Outcome::Failed(error) = fixture.detail(Duration::from_secs(2)) else {
            panic!("large stderr without XML should be a failed detail query");
        };
        assert_eq!(error.exit_code, Some(7));
        assert_eq!(error.summary, "final diagnostic marker");
    }

    #[test]
    fn output_written_after_direct_child_exit_is_excluded() {
        let fixture = QueryFixture::new();
        fixture.script(
            "(sleep 4; printf 'invalid later output\\n') & printf 'cc_library rule //app:lib\\n'; exit 0\n",
        );
        let start = Instant::now();
        let Outcome::Ok(targets) = fixture.label_kind(Duration::from_secs(2)) else {
            panic!("query should use output present at direct-child exit");
        };
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].label, "//app:lib");
    }

    #[test]
    fn descendant_held_stderr_does_not_block_detail_or_next_query() {
        let fixture = QueryFixture::new();
        fixture.script(
            "sleep 4 1>/dev/null & printf '<query><rule name=\"//app:lib\" class=\"cc_library\"/></query>'; exit 0\n",
        );
        let start = Instant::now();
        let Outcome::Ok(query) = fixture.detail(Duration::from_secs(2)) else {
            panic!("XML from the direct child should complete");
        };
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(query.rules[0].label, "//app:lib");

        fixture.script("printf '<query><rule name=\"//app:lib\" class=\"cc_library\"/></query>'\n");
        let Outcome::Ok(query) = fixture.detail(Duration::from_secs(2)) else {
            panic!("subsequent XML detail should complete");
        };
        assert_eq!(query.rules[0].label, "//app:lib");
    }

    #[test]
    fn timed_out_child_is_reaped_and_next_query_works() {
        let fixture = QueryFixture::new();
        fixture.script("exec sleep 30\n");
        let start = Instant::now();
        let Outcome::Failed(error) = fixture.label_kind(Duration::from_millis(100)) else {
            panic!("timed out query should fail");
        };
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(error.summary.contains("timed out"), "{}", error.summary);
        assert!(!error.summary.contains("exit unconfirmed"));

        fixture.script("printf 'cc_library rule //app:recovered\\n'\n");
        assert!(matches!(
            fixture.label_kind(Duration::from_secs(2)),
            Outcome::Ok(_)
        ));

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
