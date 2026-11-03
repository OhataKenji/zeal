//! Copy the standard local Bazel profile into its receiving attempt.
use super::securefs::Dir;
use crate::profile::COMPRESSED_LIMIT;
use crate::protocol::bep;
use rustix::fs::{Mode, OFlags};
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use url::Url;

pub(super) fn import(logs: &bep::BuildToolLogs, dir: &Dir) -> io::Result<()> {
    let Some(uri) = logs.log.iter().find_map(|log| match &log.file {
        Some(bep::file::File::Uri(uri)) if log.name == "command.profile.gz" => Some(uri),
        _ => None,
    }) else {
        return Ok(());
    };
    let source = open_source(uri)?;
    let mut copy = dir.file("profile.tmp", true)?;
    let result = (|| {
        if io::copy(&mut source.take(COMPRESSED_LIMIT as u64 + 1), &mut copy)?
            > COMPRESSED_LIMIT as u64
        {
            return Err(io::Error::other("profile exceeds byte limit"));
        }
        copy.sync_all()?;
        dir.rename_file("profile.tmp", "profile.json.gz")
    })();
    if result.is_err() {
        let _ = dir.remove_file("profile.tmp");
    }
    result
}

fn open_source(uri: &str) -> io::Result<File> {
    let path = Url::parse(uri)
        .ok()
        .and_then(|url| url.to_file_path().ok())
        .ok_or_else(|| io::Error::other("profile must reference a local file"))?
        .canonicalize()?;
    // Resolve Bazel's latest-profile symlink, then check and read the same FD.
    // NONBLOCK prevents a replaced source from hanging on a FIFO open.
    let file = File::from(rustix::fs::open(
        &path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(io::Error::other("profile must be an owned regular file"));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::events::test_support::Fixture;

    #[test]
    fn refuses_nonlocal_uris() {
        assert!(open_source("https://example.com/profile.gz").is_err());
        assert!(open_source("file://example.com/profile.gz").is_err());
    }

    #[test]
    fn refuses_fifo_without_waiting_for_writer() {
        let fixture = Fixture::new();
        let path = fixture.root.join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(open_source(Url::from_file_path(path).unwrap().as_str()).is_err());
    }

    #[test]
    fn oversized_profile_leaves_no_saved_or_partial_copy() {
        let fixture = Fixture::new();
        let source = fixture.root.join("source");
        File::create(&source)
            .unwrap()
            .set_len(COMPRESSED_LIMIT as u64 + 1)
            .unwrap();
        let logs = bep::BuildToolLogs {
            log: vec![bep::File {
                name: "command.profile.gz".into(),
                file: Some(bep::file::File::Uri(
                    Url::from_file_path(source).unwrap().into(),
                )),
                ..Default::default()
            }],
        };
        let dir = Dir::root(&fixture.root.join("attempt")).unwrap();
        assert!(import(&logs, &dir).is_err());
        assert!(!dir.exists("profile.json.gz").unwrap());
        assert!(!dir.exists("profile.tmp").unwrap());
    }
}
