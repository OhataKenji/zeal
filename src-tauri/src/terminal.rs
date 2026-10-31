//! Unix PTY ownership and durable output. A Console view never owns the child.
use crate::state::Core;
use rustix::event::{self, PollFd, PollFlags, Timespec};
use rustix::fs::{self, OFlags};
use rustix::termios::{self, Winsize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
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

#[tracing::instrument(target = "zeal::operations", name = "bazel.spawn", skip_all)]
pub fn spawn(command: &mut Command, dir: &Path) -> io::Result<(Child, Control, Capture)> {
    let mut log = OpenOptions::new()
        .append(true)
        .open(dir.join("terminal.log"))?;
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
mod tests;
