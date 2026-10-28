//! File access beneath an owned directory. Every child is opened relative to a
//! directory FD, without following links; special files never reach a reader.
//! rustix borrows directory descriptors and returns owned file descriptors.
use rustix::fs::{self as rustix_fs, AtFlags, Mode, OFlags};
use std::ffi::{CString, OsStr};
use std::fs::{self, File};
use std::io;
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
    if meta.uid() != rustix::process::geteuid().as_raw()
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
        if create
            && let Err(error) = rustix_fs::mkdirat(parent, &name, Mode::from_raw_mode(0o700))
            && (exclusive || error != rustix::io::Errno::EXIST)
        {
            return Err(error.into());
        }
        let fd = File::from(rustix_fs::openat(
            parent,
            &name,
            OFlags::RDONLY
                | OFlags::DIRECTORY
                | OFlags::NOFOLLOW
                | OFlags::CLOEXEC
                | OFlags::NONBLOCK,
            Mode::empty(),
        )?);
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
        let flags = OFlags::NOFOLLOW
            | OFlags::CLOEXEC
            | OFlags::NONBLOCK
            | if create {
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL
            } else {
                OFlags::RDONLY
            };
        let file = File::from(rustix_fs::openat(
            &self.fd,
            &child,
            flags,
            Mode::from_raw_mode(0o600),
        )?);
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
        match rustix_fs::statat(&self.fd, &child, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(io::Error::from(error)),
        }
    }
    pub(super) fn remove_file(&self, child: &str) -> io::Result<()> {
        self.remove(child, AtFlags::empty())
    }
    pub(super) fn remove_dir(&self, child: &str) -> io::Result<()> {
        self.remove(child, AtFlags::REMOVEDIR)
    }
    fn remove(&self, child: &str, flags: AtFlags) -> io::Result<()> {
        let child = name(OsStr::new(child))?;
        rustix_fs::unlinkat(&self.fd, &child, flags)?;
        self.sync()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fixture {
        root: PathBuf,
        dir: Dir,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-securefs-{}", uuid::Uuid::new_v4()));
            let dir = Dir::root(&root).unwrap();
            Self { root, dir }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn rejects_symlinked_directory_and_file_and_detects_dangling_link() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.root.join("real-dir")).unwrap();
        fs::write(fixture.root.join("secret"), b"secret").unwrap();
        symlink("real-dir", fixture.root.join("linked-dir")).unwrap();
        symlink("secret", fixture.root.join("linked-file")).unwrap();
        symlink("missing", fixture.root.join("dangling")).unwrap();

        assert!(fixture.dir.child("linked-dir", false).is_err());
        assert!(fixture.dir.artifact("linked-file").is_err());
        assert!(fixture.dir.exists("dangling").unwrap());
        assert!(!fixture.dir.exists("missing").unwrap());
    }

    #[test]
    fn rejects_traversal_hard_links_and_fifo_files() {
        let fixture = Fixture::new();
        assert!(fixture.dir.child("../outside", false).is_err());
        assert!(fixture.dir.file("nested/name", false).is_err());

        drop(fixture.dir.file("one-link", true).unwrap());
        fs::hard_link(
            fixture.root.join("one-link"),
            fixture.root.join("two-links"),
        )
        .unwrap();
        assert!(fixture.dir.artifact("one-link").is_err());
        assert!(fixture.dir.artifact("two-links").is_err());

        assert!(
            std::process::Command::new("mkfifo")
                .arg(fixture.root.join("pipe"))
                .status()
                .unwrap()
                .success()
        );
        assert!(fixture.dir.file("pipe", false).is_err());
    }

    #[test]
    fn removes_file_and_directory_entries() {
        let fixture = Fixture::new();
        let child_dir = fixture.dir.create_child("child").unwrap();
        drop(child_dir);
        fixture.dir.remove_dir("child").unwrap();

        drop(fixture.dir.file("file", true).unwrap());
        fixture.dir.remove_file("file").unwrap();
        assert!(!fixture.dir.exists("child").unwrap());
        assert!(!fixture.dir.exists("file").unwrap());
    }
}
