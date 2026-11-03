//! Unix PTY ownership and captured output. A Console view never owns the child.
use crate::state::Core;
use rustix::event::{self, PollFd, PollFlags, Timespec};
use rustix::fs::{self, OFlags};
use rustix::termios::{self, Winsize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct Control(File);

#[cfg(test)]
impl Control {
    /// A detached control for tests that never exercise the terminal.
    pub fn stub() -> Self {
        Self(File::open("/dev/null").expect("/dev/null"))
    }
}

impl Control {
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.0.write(bytes) {
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(0)
            }
            result => result,
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        if !(2..=1000).contains(&cols) || !(1..=1000).contains(&rows) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid terminal size",
            ));
        }
        let size = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        Ok(termios::tcsetwinsize(&self.0, size)?)
    }

    pub fn foreground_group(&self) -> Option<i32> {
        termios::tcgetpgrp(&self.0)
            .ok()
            .map(|group| group.as_raw_pid())
    }
}

pub struct Capture {
    done: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

impl Capture {
    #[must_use]
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub fn finish(&mut self) -> io::Result<()> {
        self.done.store(true, Ordering::Release);
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| io::Error::other("terminal capture failed"))?
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// The console session's captured output: a temp file unlinked on creation,
/// so it disappears once every handle closes.
pub fn session_log() -> io::Result<File> {
    let path = std::env::temp_dir().join(format!("zeal-{}.log", uuid::Uuid::new_v4()));
    let log = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    Ok(log)
}

#[tracing::instrument(target = "zeal::operations", name = "bazel.spawn", skip_all)]
pub fn spawn(command: &mut Command, log: &File) -> io::Result<(Child, Control, Capture)> {
    let mut log = log.try_clone()?;
    let size = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // Both descriptors are owned and close-on-exec, including on error paths.
    let pty = rustix_openpty::openpty(None, Some(&size))?;
    let master = File::from(pty.controller);
    let slave = File::from(pty.user);
    // The shared master description is nonblocking for both capture and input.
    let flags = fs::fcntl_getfl(&master)?;
    fs::fcntl_setfl(&master, flags | OFlags::NONBLOCK)?;
    let mut reader = master.try_clone()?;
    command
        .stdin(slave.try_clone()?)
        .stdout(slave.try_clone()?)
        .stderr(slave)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor");
    // A controlling terminal must be set in the child, after fork and before
    // exec. Command has no safe hook for this; process_group alone cannot
    // create a session or acquire its controlling terminal.
    // SAFETY: the hook uses only async-signal-safe syscalls and errno retrieval,
    // with no allocation or locks. Command has already duplicated stdio to
    // 0/1/2, so fd 0 is the live slave. All state changes affect only the child.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0
                || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0
                || libc::tcsetpgrp(0, libc::getpid()) < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let done = Arc::new(AtomicBool::new(false));
    let finished = done.clone();
    let failed = Arc::new(AtomicBool::new(false));
    let failed_worker = failed.clone();
    let worker = thread::Builder::new()
        .name("pty-output".into())
        .spawn(move || {
            let result = (|| {
                let mut buffer = [0; 64 * 1024];
                let mut draining = None;
                loop {
                    if finished.load(Ordering::Acquire) {
                        let since = draining.get_or_insert_with(Instant::now);
                        // Descendants retaining the slave must not keep an ended invocation alive.
                        if since.elapsed() >= Duration::from_millis(100) {
                            break;
                        }
                    }
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => log.write_all(&buffer[..count])?,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            if draining.is_some() {
                                break;
                            }
                            let mut fds = [PollFd::new(&reader, PollFlags::IN)];
                            let timeout = Timespec {
                                tv_sec: 0,
                                tv_nsec: 20_000_000,
                            };
                            let _ = event::poll(&mut fds, Some(&timeout));
                        }
                        // Linux reports EIO when the slave closes; macOS returns EOF.
                        Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                        Err(e) => return Err(e),
                    }
                }
                log.sync_all()
            })();
            failed_worker.store(result.is_err(), Ordering::Release);
            result
        });
    match worker {
        Ok(worker) => Ok((
            child,
            Control(master),
            Capture {
                done,
                failed,
                worker: Some(worker),
            },
        )),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

pub fn write(core: &Core, id: &str, bytes: &[u8]) -> Result<usize, String> {
    if bytes.len() > 64 * 1024 {
        return Err("terminal input chunk is too large".into());
    }
    let mut st = core.state.lock().expect("application state lock poisoned");
    let Some(invocation) = st.active_invocation.as_mut() else {
        return Err("terminal is no longer running".into());
    };
    if invocation.id.as_str() != id || invocation.pgid.is_none() || invocation.stopping {
        return Err("terminal is no longer running".into());
    }
    invocation.terminal.write(bytes).map_err(|e| e.to_string())
}

pub fn resize(core: &Core, id: &str, cols: u16, rows: u16) -> Result<(), String> {
    let st = core.state.lock().expect("application state lock poisoned");
    let Some(invocation) = st.active_invocation.as_ref() else {
        return Ok(());
    };
    if invocation.id.as_str() != id || invocation.pgid.is_none() {
        return Ok(());
    }
    invocation
        .terminal
        .resize(cols, rows)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ActiveInvocation;

    struct Fixture {
        log: File,
        id: String,
        child: Child,
        capture: Capture,
        core: Core,
    }
    impl Fixture {
        fn new(script: &str) -> Self {
            let log = session_log().unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let (child, control, capture) =
                spawn(Command::new("sh").args(["-c", script]), &log).unwrap();
            let core = Core::new();
            let mut st = core.state.lock().unwrap();
            st.active_invocation = Some(ActiveInvocation {
                id: id.clone().into(),
                terminal: control,
                pgid: Some(child.id() as i32),
                stopping: false,
                stop_started: None,
            });
            drop(st);
            Self {
                log,
                id,
                child,
                capture,
                core,
            }
        }
        fn output(&self) -> String {
            String::from_utf8(read_log(&self.log)).unwrap()
        }
        fn wait_for(&self, text: &str) {
            wait_for_output(self, text);
        }
        fn send(&self, data: &[u8]) {
            assert_eq!(write(&self.core, &self.id, data).unwrap(), data.len());
        }
        fn finish(&mut self) -> i32 {
            let code = wait_for_exit(&mut self.child, &self.log);
            self.capture.finish().unwrap();
            _ = self
                .core
                .state
                .lock()
                .unwrap()
                .active_invocation
                .as_mut()
                .map(|invocation| invocation.pgid = None);
            code
        }
    }
    fn wait_for_output(fixture: &Fixture, text: &str) {
        let until = Instant::now() + Duration::from_secs(5);
        while !fixture.output().contains(text) {
            assert!(
                Instant::now() < until,
                "waiting for {text}: {}",
                fixture.output()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn read_log(log: &File) -> Vec<u8> {
        use std::os::unix::fs::FileExt;
        let mut bytes = vec![0; log.metadata().unwrap().len() as usize];
        log.read_exact_at(&mut bytes, 0).unwrap();
        bytes
    }

    fn wait_for_exit(child: &mut Child, log: &File) -> i32 {
        let until = Instant::now() + Duration::from_secs(5);
        let mut status = child.try_wait().unwrap();
        while status.is_none() {
            assert!(
                Instant::now() < until,
                "child did not exit: {}",
                String::from_utf8_lossy(&read_log(log))
            );
            thread::sleep(Duration::from_millis(10));
            status = child.try_wait().unwrap();
        }
        status.unwrap().code().unwrap_or(-1)
    }

    fn kill_if_running(child: &mut Child) {
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            kill_if_running(&mut self.child);
            let _ = self.capture.finish();
        }
    }

    #[test]
    fn unattached_terminal_has_no_foreground_group() {
        let pty = rustix_openpty::openpty(None, None).unwrap();
        let control = Control(File::from(pty.controller));
        assert_eq!(control.foreground_group(), None);
    }

    #[test]
    fn foreground_group_is_cleared_when_session_leader_exits() {
        let mut f = Fixture::new("printf 'ready>'; read value; exit 0");
        f.wait_for("ready>");
        assert_eq!(
            f.core
                .state
                .lock()
                .unwrap()
                .active_invocation
                .as_ref()
                .unwrap()
                .terminal
                .foreground_group(),
            Some(f.child.id() as i32)
        );
        f.send(b"done\r");
        assert_eq!(f.finish(), 0);
        assert_eq!(
            f.core
                .state
                .lock()
                .unwrap()
                .active_invocation
                .as_ref()
                .unwrap()
                .terminal
                .foreground_group(),
            None
        );
    }

    #[test]
    fn interactive_process_has_a_controlling_terminal_and_resizes() {
        let mut f = Fixture::new(
            "test -t 0 && test -t 1 && test -t 2 || exit 9; printf 'ready>'; read value; stty size; printf 'answer=%s\\n' $((value * 2))",
        );
        f.wait_for("ready>");
        assert!(write(&f.core, "old-invocation", b"999\n").is_err());
        resize(&f.core, &f.id, 120, 40).unwrap();
        f.send(b"21\r");
        assert_eq!(f.finish(), 0);
        assert!(f.output().contains("40 120"));
        assert!(f.output().contains("answer=42"));
        assert!(write(&f.core, &f.id, b"late").is_err());
    }

    #[test]
    fn raw_arrow_keys_do_not_need_enter_and_escape_output_is_saved() {
        let mut f = Fixture::new(
            "stty -echo -icanon min 1 time 0; printf '\\033[2J\\033[Hready'; od -An -t x1 -N 3",
        );
        f.wait_for("ready");
        f.send(b"\x1b[A");
        assert_eq!(f.finish(), 0);
        assert!(
            f.output()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains("1b 5b 41"),
            "{}",
            f.output()
        );
        assert!(f.output().contains("\x1b[2J\x1b[H"));
    }

    #[test]
    fn control_c_signals_the_foreground_process() {
        let mut f =
            Fixture::new("trap 'printf interrupted; exit 130' INT; printf ready; read value");
        f.wait_for("ready");
        f.send(b"\x03");
        assert_eq!(f.finish(), 130);
        assert!(f.output().contains("interrupted"));
    }

    #[test]
    fn control_d_delivers_terminal_eof() {
        let mut f = Fixture::new("printf ready; read value; printf 'eof=%s' $?");
        f.wait_for("ready");
        f.send(b"\x04");
        assert_eq!(f.finish(), 0);
        assert!(f.output().contains("eof=1"));
    }
}
