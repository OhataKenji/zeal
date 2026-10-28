//! Unix PTY ownership and durable output. A Console view never owns the child.
// SAFETY: PTY setup and child session configuration require libc calls that
// safe standard-library APIs do not expose. Successful raw descriptors are
// converted to `File` exactly once, which then owns them through RAII.
use crate::state::Core;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct Control(File);

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
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: ioctl reads the initialized winsize; this file owns the PTY.
        if unsafe { libc::ioctl(self.0.as_raw_fd(), libc::TIOCSWINSZ, &size) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn foreground_group(&self) -> Option<i32> {
        // SAFETY: tcgetpgrp only inspects this PTY's foreground process group.
        let group = unsafe { libc::tcgetpgrp(self.0.as_raw_fd()) };
        (group > 0).then_some(group)
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

#[tracing::instrument(target = "zeal::operations", name = "bazel.spawn", skip_all)]
pub fn spawn(command: &mut Command, dir: &Path) -> io::Result<(Child, Control, Capture)> {
    let mut log = OpenOptions::new().append(true).open(dir.join("log.txt"))?;
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: output descriptors and winsize are valid; no name buffer is used.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty returned two new, owned descriptors.
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    for file in [&master, &slave] {
        // SAFETY: fcntl changes descriptor flags only.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // The shared master description is nonblocking for both capture and input.
    // SAFETY: `master` remains an owned, live descriptor; F_GETFL reads flags.
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `master` remains live and F_SETFL receives an integer flag value.
    if unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut reader = master.try_clone()?;
    command
        .stdin(slave.try_clone()?)
        .stdout(slave.try_clone()?)
        .stderr(slave)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor");
    // SAFETY: the child hook calls only async-signal-safe syscalls. Stdio has
    // already been duplicated to 0/1/2 by Command when this hook runs.
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
                        // Descendants retaining the slave must not keep an ended run alive.
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
                            let mut fd = libc::pollfd {
                                fd: reader.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            };
                            // SAFETY: poll receives one initialized descriptor and a bounded timeout.
                            unsafe {
                                libc::poll(&mut fd, 1, 20);
                            }
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
    if st.run.as_deref() != Some(id) || st.run_pgid.is_none() || st.run_stopping {
        return Err("terminal is no longer running".into());
    }
    st.run_terminal
        .as_mut()
        .ok_or("no terminal")?
        .write(bytes)
        .map_err(|e| e.to_string())
}

pub fn resize(core: &Core, id: &str, cols: u16, rows: u16) -> Result<(), String> {
    let st = core.state.lock().expect("application state lock poisoned");
    if st.run.as_deref() != Some(id) || st.run_pgid.is_none() {
        return Ok(());
    }
    st.run_terminal
        .as_ref()
        .ok_or("no terminal")?
        .resize(cols, rows)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

    struct Fixture {
        root: std::path::PathBuf,
        dir: std::path::PathBuf,
        record: store::RunRecord,
        child: Child,
        capture: Capture,
        core: Core,
    }
    impl Fixture {
        fn new(script: &str) -> Self {
            let root = std::env::temp_dir().join(format!("zeal-pty-{}", uuid::Uuid::new_v4()));
            let ws = root.join("workspace");
            let data = root.join("data");
            std::fs::create_dir_all(&ws).unwrap();
            let (record, dir) = store::create(&data, &ws, "run", "//:test").unwrap();
            let (child, control, capture) =
                spawn(Command::new("sh").args(["-c", script]), &dir).unwrap();
            let core = Core::new();
            {
                let mut st = core.state.lock().unwrap();
                st.run = Some(record.id.clone().into());
                st.run_pgid = Some(child.id() as i32);
                st.run_terminal = Some(control);
            }
            Self {
                root,
                dir,
                record,
                child,
                capture,
                core,
            }
        }
        fn output(&self) -> String {
            std::fs::read_to_string(self.dir.join("log.txt")).unwrap()
        }
        fn wait_for(&self, text: &str) {
            let until = Instant::now() + Duration::from_secs(5);
            while !self.output().contains(text) {
                assert!(
                    Instant::now() < until,
                    "waiting for {text}: {}",
                    self.output()
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
        fn send(&self, data: &[u8]) {
            assert_eq!(
                write(&self.core, &self.record.id, data).unwrap(),
                data.len()
            );
        }
        fn finish(&mut self) -> i32 {
            let until = Instant::now() + Duration::from_secs(5);
            let code = loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status.code().unwrap_or(-1);
                }
                assert!(
                    Instant::now() < until,
                    "child did not exit: {}",
                    self.output()
                );
                thread::sleep(Duration::from_millis(10));
            };
            self.capture.finish().unwrap();
            self.core.state.lock().unwrap().run_pgid = None;
            code
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            let _ = self.capture.finish();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn interactive_process_has_a_controlling_terminal_and_resizes() {
        let mut f = Fixture::new(
            "test -t 0 && test -t 1 && test -t 2 || exit 9; printf 'ready>'; read value; stty size; printf 'answer=%s\\n' $((value * 2))",
        );
        f.wait_for("ready>");
        assert!(write(&f.core, "old-run", b"999\n").is_err());
        resize(&f.core, &f.record.id, 120, 40).unwrap();
        f.send(b"21\r");
        assert_eq!(f.finish(), 0);
        assert!(f.output().contains("40 120"));
        assert!(f.output().contains("answer=42"));
        assert!(write(&f.core, &f.record.id, b"late").is_err());
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
