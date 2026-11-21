//! Bounded `bazel` subprocesses shared by `info` and `mod` commands.
//!
//! Both pipes drain on scoped threads behind one deadline: stdout keeps a
//! bounded head, stderr a bounded tail. [`DeadlinePipe`] polls each read
//! against the shared deadline so a descendant holding a pipe open cannot
//! keep reader threads or descriptors alive past it. Nonzero exits are
//! errors carrying the stderr tail; callers only see successful output.

use rustix::event::{self, PollFd, PollFlags, Timespec};
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// stderr is kept as a bounded tail: Bazel prints `ERROR:` at the end.
const STDERR_TAIL_CAP: usize = 8 * 1024;

/// A successful bounded run: head-capped stdout plus a truncation flag.
#[derive(Debug)]
pub(crate) struct BoundedOutput {
    pub stdout: Vec<u8>,
    pub stdout_truncated: bool,
}

/// A pipe whose reads fail with [`io::ErrorKind::TimedOut`] once `deadline`
/// passes, even when a descendant still holds the write end open.
struct DeadlinePipe<R>(R, Instant);

impl<R: Read + AsFd> Read for DeadlinePipe<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self
            .1
            .checked_duration_since(Instant::now())
            .ok_or(io::ErrorKind::TimedOut)?;
        let timeout = Timespec::try_from(left).expect("bounded timeout");
        let mut fds = [PollFd::new(&self.0, PollFlags::IN)];
        if event::poll(&mut fds, Some(&timeout))? == 0 {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.0.read(buf)
    }
}

/// Drain `pipe` in 8 KiB chunks until EOF or error, handing each chunk to
/// `on_chunk`. `Interrupted` reads retry; every other error propagates.
fn read_chunks(pipe: &mut impl Read, mut on_chunk: impl FnMut(&[u8])) -> io::Result<()> {
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
            Ok(n) => on_chunk(&chunk[..n]),
        }
    }
    Ok(())
}

/// Keep the first `cap` bytes of the stream and drain the rest: the child
/// must never block on a full pipe. Returns the bytes and whether any were
/// dropped.
pub(crate) fn capture_head(pipe: &mut impl Read, cap: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut buf = Vec::new();
    let mut truncated = false;
    read_chunks(pipe, |chunk| {
        let kept = chunk.len().min(cap - buf.len());
        buf.extend_from_slice(&chunk[..kept]);
        truncated |= kept < chunk.len();
    })?;
    Ok((buf, truncated))
}

/// Keep the last `cap` bytes of the stream.
pub(crate) fn capture_tail(pipe: &mut impl Read, cap: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    read_chunks(pipe, |chunk| {
        buf.extend_from_slice(chunk);
        buf.drain(..buf.len().saturating_sub(cap));
    })?;
    Ok(buf)
}

/// Run `bazel --noblock_for_lock <args>` in `ws` with bounded IO and one
/// deadline covering spawn, exit and pipe drain. `cancelled` is checked
/// before spawning and while waiting; a set flag kills the confirmed
/// unreaped child and ends the run early.
pub(crate) fn run_bazel_bounded(
    bazel: &Path,
    ws: &Path,
    args: &[&str],
    deadline: Instant,
    stdout_cap: usize,
    cancelled: &AtomicBool,
) -> Result<BoundedOutput, String> {
    let label = command_label(args);
    let mut cmd = Command::new(bazel);
    cmd.arg("--noblock_for_lock")
        .args(args)
        .current_dir(ws)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.process_group(0);
    if cancelled.load(Ordering::Relaxed) {
        return Err("application is shutting down".to_string());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start {label}: {e}"))?;
    let pid = child.id();
    let out_pipe = child.stdout.take().expect("stdout piped");
    let err_pipe = child.stderr.take().expect("stderr piped");

    thread::scope(|s| {
        let out = s.spawn(move || {
            let mut pipe = DeadlinePipe(out_pipe, deadline);
            capture_head(&mut pipe, stdout_cap)
        });
        let err = s.spawn(move || {
            let mut pipe = DeadlinePipe(err_pipe, deadline);
            capture_tail(&mut pipe, STDERR_TAIL_CAP)
        });
        // `?` still joins both readers on the way out; DeadlinePipe ends them
        // at the deadline even when a descendant keeps a pipe open. Once
        // `wait_with_timeout` returns, the child is reaped and its pid — and
        // process group id — may be recycled, so nothing may be signalled.
        let status = wait_with_timeout(&mut child, pid, deadline, &label, cancelled)?;
        let (stdout, stdout_truncated) = join_reader(out, &label, "stdout")?;
        let tail = join_reader(err, &label, "stderr")?;
        if status.success() {
            return Ok(BoundedOutput {
                stdout,
                stdout_truncated,
            });
        }
        let detail = utf8_tail(&tail).trim().to_string();
        Err(if detail.is_empty() {
            format!("{label} exited with {status}")
        } else {
            format!("{label} failed: {detail}")
        })
    })
}

fn join_reader<T>(
    handle: thread::ScopedJoinHandle<'_, io::Result<T>>,
    label: &str,
    pipe: &str,
) -> Result<T, String> {
    handle
        .join()
        .map_err(|_| format!("{label} {pipe} reader panicked"))?
        .map_err(|e| {
            if e.kind() == io::ErrorKind::TimedOut {
                format!("{label} timed out")
            } else {
                format!("{label} {pipe} read failed: {e}")
            }
        })
}

/// `bazel mod graph ...`-style label for error messages: the leading
/// non-flag arguments.
fn command_label(args: &[&str]) -> String {
    let words: Vec<&str> = args
        .iter()
        .copied()
        .take_while(|arg| !arg.starts_with('-'))
        .filter(|arg| !arg.is_empty())
        .collect();
    format!("bazel {}", words.join(" "))
}

/// Decode stderr tail bytes as UTF-8 without a leading replacement char: the
/// 8 KiB cut can split a codepoint, so leading continuation bytes are dropped.
fn utf8_tail(bytes: &[u8]) -> String {
    let start = bytes
        .iter()
        .position(|b| (b & 0xC0) != 0x80)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn wait_with_timeout(
    child: &mut Child,
    pid: u32,
    deadline: Instant,
    label: &str,
    cancelled: &AtomicBool,
) -> Result<ExitStatus, String> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // A try_wait error means ownership is unknown; a numeric process
            // group must never be signalled for a pid that may already have
            // been recycled.
            Err(e) => return Err(format!("failed waiting for {label}: {e}")),
            Ok(None) => {}
        }
        // Ok(None) above confirms the child is still running and unreaped,
        // so its process group is safe to signal on either early exit.
        if cancelled.load(Ordering::Relaxed) {
            kill_group_and_reap(child, pid);
            return Err(format!("{label} cancelled: application is shutting down"));
        }
        if Instant::now() >= deadline {
            kill_group_and_reap(child, pid);
            return Err(format!("{label} timed out"));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn kill_group_and_reap(child: &mut Child, pid: u32) {
    use rustix::process::{Pid, Signal, kill_process_group};
    // Only reached while the child is confirmed unreaped, so its pid —
    // and process group id — cannot have been recycled. ESRCH means the
    // group already exited; child.kill()/wait() below still cover the
    // leader itself.
    if let Ok(raw) = i32::try_from(pid)
        && raw > 1
        && let Some(pgid) = Pid::from_raw(raw)
    {
        let _ = kill_process_group(pgid, Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::test_support::{TempDir, fake_bazel};

    fn fake_ws(dir: &TempDir) -> std::path::PathBuf {
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        ws
    }

    /// A bounded run with a generous deadline and no cancellation.
    fn run(bazel: &Path, ws: &Path, args: &[&str], cap: usize) -> Result<BoundedOutput, String> {
        run_bazel_bounded(
            bazel,
            ws,
            args,
            Instant::now() + Duration::from_secs(10),
            cap,
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn run_bazel_bounded_captures_stdout_and_reports_failures() {
        let dir = TempDir::new();
        let ws = fake_ws(&dir);
        let ok = fake_bazel(dir.path(), "bazel-ok", "#!/bin/sh\nprintf 'out-data'\n");
        let out = run(&ok, &ws, &["info"], 1024).unwrap();
        assert_eq!(out.stdout, b"out-data");
        assert!(!out.stdout_truncated);

        let fail = fake_bazel(
            dir.path(),
            "bazel-fail",
            "#!/bin/sh\nprintf 'err-data' >&2\nexit 3\n",
        );
        let err = run(&fail, &ws, &["info"], 1024).unwrap_err();
        assert!(err.contains("err-data"), "{err}");
    }

    #[test]
    fn run_bazel_bounded_times_out_and_kills_the_process_group() {
        let dir = TempDir::new();
        let ws = fake_ws(&dir);
        // `sleep` runs in the child's process group; the group kill must reap
        // it too, so the call returns promptly instead of waiting 30 s.
        let bazel = fake_bazel(dir.path(), "bazel", "#!/bin/sh\nsleep 30\n");
        let started = Instant::now();
        let err = run_bazel_bounded(
            &bazel,
            &ws,
            &["info"],
            Instant::now() + Duration::from_millis(200),
            1024,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn deadline_pipe_times_out_while_the_write_end_is_held() {
        let (reader, _writer) = std::io::pipe().unwrap();
        let mut pipe = DeadlinePipe(reader, Instant::now() + Duration::from_millis(50));
        let started = Instant::now();
        let err = pipe.read(&mut [0u8; 8]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn run_bazel_bounded_marks_stdout_truncated_at_the_cap() {
        let dir = TempDir::new();
        let ws = fake_ws(&dir);
        let script = "#!/bin/sh\nprintf '%*s' 24 '' | tr ' ' 'a'\n";
        let bazel = fake_bazel(dir.path(), "bazel", script);
        let out = run(&bazel, &ws, &["info"], 16).unwrap();
        assert_eq!(out.stdout, vec![b'a'; 16]);
        assert!(out.stdout_truncated);

        // Exactly at the cap is not truncated.
        let script = "#!/bin/sh\nprintf '%*s' 16 '' | tr ' ' 'a'\n";
        let bazel = fake_bazel(dir.path(), "bazel2", script);
        let out = run(&bazel, &ws, &["info"], 16).unwrap();
        assert_eq!(out.stdout.len(), 16);
        assert!(!out.stdout_truncated);
    }

    #[test]
    fn nonzero_exit_reports_a_utf8_safe_stderr_tail() {
        let dir = TempDir::new();
        let ws = fake_ws(&dir);
        // 3000 '€' signs overflow the 8 KiB tail; the window cut lands
        // mid-codepoint and must not leak a leading U+FFFD.
        let script = format!("#!/bin/sh\nprintf '{}' >&2\nexit 1\n", "€".repeat(3000));
        let bazel = fake_bazel(dir.path(), "bazel", &script);
        let err = run(&bazel, &ws, &["info"], 1024).unwrap_err();
        assert!(err.contains('€'), "{err}");
        assert!(!err.contains('\u{FFFD}'), "{err}");
    }

    #[test]
    fn run_bazel_bounded_fails_when_bazel_cannot_start() {
        let dir = TempDir::new();
        let ws = fake_ws(&dir);
        let err = run(
            std::path::Path::new("/nonexistent/zeal-bazel"),
            &ws,
            &["info"],
            1024,
        )
        .unwrap_err();
        assert!(err.contains("failed to start bazel info"), "{err}");
    }
}
