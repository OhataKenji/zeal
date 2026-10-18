//! File access beneath an owned directory. Every child is opened relative to a
//! directory FD, without following links; special files never reach a reader.
use std::ffi::{CString, OsStr};
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(super) struct Dir {
    fd: File,
    path: PathBuf,
}

fn name(value: &OsStr) -> io::Result<CString> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid child name",
        ));
    }
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid child name"))
}

fn owned(file: &File, directory: bool) -> io::Result<()> {
    let meta = file.metadata()?;
    // The same FD is used for checking and reading, including FIFO rejection.
    if meta.uid() != unsafe { libc::geteuid() }
        || (directory && !meta.is_dir())
        || (!directory && (!meta.is_file() || meta.nlink() != 1))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe file ownership or type",
        ));
    }
    Ok(())
}

// Existing system ancestors may contain platform links (for example /var on
// macOS). Resolve those once. Create each missing ancestor through its parent
// FD so its directory entry is durable before creating the next level.
fn parent_directory(path: &Path) -> io::Result<(File, PathBuf)> {
    match path.canonicalize() {
        Ok(canonical) => Ok((File::open(&canonical)?, canonical)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "missing directory parent")
                })?;
            let child = path.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid directory name")
            })?;
            let (parent_fd, parent_path) = parent_directory(parent)?;
            let created = Dir::at(&parent_fd, &parent_path, child, true, false)?;
            Ok((created.fd, created.path))
        }
        Err(error) => Err(error),
    }
}

impl Dir {
    pub(super) fn root(path: &Path) -> io::Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "data directory must have a parent",
                )
            })?;
        let (parent_fd, parent) = parent_directory(parent)?;
        let base = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid data directory"))?;
        Self::at(&parent_fd, &parent, base, true, false)
    }

    fn at(
        parent: &File,
        path: &Path,
        child: &OsStr,
        create: bool,
        exclusive: bool,
    ) -> io::Result<Self> {
        let name = name(child)?;
        if create {
            let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
            if result != 0 {
                let error = io::Error::last_os_error();
                if exclusive || error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
        let raw = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY
                    | libc::O_DIRECTORY
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { File::from_raw_fd(raw) };
        owned(&fd, true)?;
        fd.set_permissions(fs::Permissions::from_mode(0o700))?;
        if create {
            // Sync every link in the path before exposing a usable child. This
            // also covers an existing entry left by an earlier failed sync.
            fd.sync_all()?;
            parent.sync_all()?;
        }
        Ok(Self {
            fd,
            path: path.join(child),
        })
    }

    pub(super) fn child(&self, child: &str, create: bool) -> io::Result<Self> {
        Self::at(&self.fd, &self.path, OsStr::new(child), create, false)
    }
    pub(super) fn create_child(&self, child: &str) -> io::Result<Self> {
        Self::at(&self.fd, &self.path, OsStr::new(child), true, true)
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn sync(&self) -> io::Result<()> {
        self.fd.sync_all()
    }

    pub(super) fn file(&self, child: &str, create: bool) -> io::Result<File> {
        self.open_file(child, create, true)
    }
    pub(super) fn artifact(&self, child: &str) -> io::Result<File> {
        self.open_file(child, false, false)
    }
    fn open_file(&self, child: &str, create: bool, private: bool) -> io::Result<File> {
        let child = name(OsStr::new(child))?;
        let flags = libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if create {
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
            } else {
                libc::O_RDONLY
            };
        let raw = unsafe { libc::openat(self.fd.as_raw_fd(), child.as_ptr(), flags, 0o600) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(raw) };
        owned(&file, false)?;
        // Do not change permissions on existing artifacts. Refuse unsafe files.
        if private && file.metadata()?.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe file permissions",
            ));
        }
        Ok(file)
    }

    pub(super) fn exists(&self, child: &str) -> io::Result<bool> {
        let child = name(OsStr::new(child))?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                self.fd.as_raw_fd(),
                child.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            Ok(true)
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(err)
            }
        }
    }
    pub(super) fn remove_file(&self, child: &str) -> io::Result<()> {
        self.remove(child, 0)
    }
    pub(super) fn remove_dir(&self, child: &str) -> io::Result<()> {
        self.remove(child, libc::AT_REMOVEDIR)
    }
    fn remove(&self, child: &str, flag: i32) -> io::Result<()> {
        let child = name(OsStr::new(child))?;
        if unsafe { libc::unlinkat(self.fd.as_raw_fd(), child.as_ptr(), flag) } != 0 {
            return Err(io::Error::last_os_error());
        }
        self.sync()
    }
}
