//! Import an optional local Bazel profile into its receiving attempt.
use super::securefs::Dir;
use crate::profile::{COMPRESSED_LIMIT, EXPANDED_LIMIT};
use crate::protocol::bep;
use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use rustix::fs::{Mode, OFlags};
use serde::Deserialize;
use serde::de::{self, IgnoredAny, SeqAccess, Visitor};
use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use url::Url;

const PROFILE: &str = "profile.json.gz";
const TEMP: &str = ".profile.json.gz.tmp";

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Capture the profile before acknowledging its BEP stream. The source is never
/// modified; only a complete, matching profile becomes visible to readers.
pub(super) fn import(logs: &bep::BuildToolLogs, invocation_id: &str, dir: &Dir) -> io::Result<()> {
    let Some(uri) = logs.log.iter().find_map(|log| {
        if !matches!(
            log.name.as_str(),
            "command.profile.gz" | "command.profile.json"
        ) {
            return None;
        }
        match &log.file {
            Some(bep::file::File::Uri(uri)) => Some(uri.as_str()),
            _ => None,
        }
    }) else {
        return Ok(());
    };
    if dir.exists(PROFILE)? {
        return Ok(());
    }
    let source = open_source(uri)?;
    let mut staged = dir.file(TEMP, true)?;
    let result = (|| {
        copy_profile(source, &mut staged)?;
        staged.seek(SeekFrom::Start(0))?;
        validate(&mut staged, invocation_id, EXPANDED_LIMIT)?;
        staged.sync_all()?;
        dir.rename_file(TEMP, PROFILE)
    })();
    if result.is_err() {
        let _ = dir.remove_file(TEMP);
    }
    result
}

fn open_source(uri: &str) -> io::Result<File> {
    if uri.len() > 16 * 1024 {
        return Err(invalid("profile URI is too long"));
    }
    let url = Url::parse(uri).map_err(|_| invalid("invalid profile URI"))?;
    if url.scheme() != "file"
        || !matches!(url.host_str(), None | Some("localhost"))
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("profile URI must name a local file"));
    }
    // Bazel may publish its latest-profile symlink. Resolve it, then open the
    // result without following a replacement symlink and check that same FD.
    let path = url
        .to_file_path()
        .map_err(|_| invalid("invalid profile file path"))?
        .canonicalize()?;
    let source = File::from(rustix::fs::open(
        &path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?);
    let metadata = source.metadata()?;
    if !metadata.is_file() || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(invalid("profile source must be an owned regular file"));
    }
    Ok(source)
}

fn copy_profile(source: File, staged: &mut File) -> io::Result<()> {
    let mut source = BufReader::new(source);
    if source.fill_buf()?.starts_with(&[0x1f, 0x8b]) {
        copy_bounded(source, staged, COMPRESSED_LIMIT)?;
    } else {
        let mut encoder = GzEncoder::new(&mut *staged, Compression::default());
        copy_bounded(source, &mut encoder, EXPANDED_LIMIT)?;
        encoder.finish()?;
        if staged.metadata()?.len() > COMPRESSED_LIMIT as u64 {
            return Err(invalid("compressed profile exceeds byte limit"));
        }
    }
    staged.flush()
}

fn copy_bounded<R: Read, W: Write>(input: R, output: &mut W, limit: usize) -> io::Result<()> {
    if io::copy(&mut input.take(limit as u64 + 1), output)? > limit as u64 {
        return Err(invalid("profile exceeds byte limit"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct Metadata {
    build_id: String,
}

#[derive(Deserialize)]
struct Trace {
    #[serde(rename = "otherData")]
    metadata: Metadata,
    #[serde(rename = "traceEvents")]
    _events: Events,
}

/// Validate the array without retaining its potentially large event payloads.
struct Events;
impl<'de> Deserialize<'de> for Events {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(Events)
    }
}
impl<'de> Visitor<'de> for Events {
    type Value = Events;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("profile event array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Events, A::Error> {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Events)
    }
}

fn validate<R: Read>(input: R, invocation_id: &str, expanded_limit: usize) -> io::Result<()> {
    // Reading one extra byte distinguishes a complete document at the limit
    // from an oversized document. end() also validates gzip trailers and junk.
    let expanded = MultiGzDecoder::new(input).take(expanded_limit as u64 + 1);
    let mut reader = BufReader::new(expanded);
    let mut decoder = serde_json::Deserializer::from_reader(&mut reader);
    let trace = Trace::deserialize(&mut decoder).map_err(|_| invalid("invalid profile JSON"))?;
    decoder.end().map_err(|_| invalid("invalid profile JSON"))?;
    if reader.into_inner().limit() == 0 {
        return Err(invalid("expanded profile exceeds byte limit"));
    }
    let actual = uuid::Uuid::parse_str(&trace.metadata.build_id)
        .map_err(|_| invalid("invalid profile invocation ID"))?;
    let expected =
        uuid::Uuid::parse_str(invocation_id).map_err(|_| invalid("invalid invocation ID"))?;
    if actual != expected {
        return Err(invalid("profile belongs to another invocation"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        dir: Dir,
        id: String,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-profile-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            Self {
                dir: Dir::root(&root.join("attempt")).unwrap(),
                root,
                id: uuid::Uuid::new_v4().to_string(),
            }
        }
        fn json(&self) -> Vec<u8> {
            serde_json::to_vec(&json!({
                "otherData": {"build_id": self.id},
                "traceEvents": [{"name": "Finishing", "ts": 123}]
            }))
            .unwrap()
        }
        fn source(&self, name: &str, content: &[u8]) -> bep::BuildToolLogs {
            let path = self.root.join("source with spaces");
            fs::write(&path, content).unwrap();
            logs(name, Url::from_file_path(path).unwrap().to_string())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }
    fn logs(name: &str, uri: String) -> bep::BuildToolLogs {
        bep::BuildToolLogs {
            log: vec![bep::File {
                name: name.into(),
                file: Some(bep::file::File::Uri(uri)),
                ..Default::default()
            }],
        }
    }
    fn saved_json(dir: &Dir) -> Vec<u8> {
        let mut bytes = Vec::new();
        MultiGzDecoder::new(dir.artifact(PROFILE).unwrap())
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }

    #[test]
    fn imports_gzip_profile_without_modifying_source() {
        let fixture = Fixture::new();
        let bytes = gzip(&fixture.json());
        let logs = fixture.source("command.profile.gz", &bytes);
        let source = fixture.root.join("source with spaces");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();

        import(&logs, &fixture.id, &fixture.dir).unwrap();

        assert_eq!(saved_json(&fixture.dir), fixture.json());
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert_eq!(fs::metadata(source).unwrap().mode() & 0o777, 0o640);
        assert_eq!(
            fixture
                .dir
                .artifact(PROFILE)
                .unwrap()
                .metadata()
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
        assert!(!fixture.dir.exists(TEMP).unwrap());
    }

    #[test]
    fn imports_plain_custom_profile_as_gzip() {
        let fixture = Fixture::new();
        let logs = fixture.source("command.profile.json", &fixture.json());

        import(&logs, &fixture.id, &fixture.dir).unwrap();

        assert_eq!(saved_json(&fixture.dir), fixture.json());
    }

    #[test]
    fn resolves_bazel_latest_profile_symlink() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("original"), gzip(&fixture.json())).unwrap();
        let path = fixture.root.join("command.profile.gz");
        symlink("original", &path).unwrap();
        let logs = logs(
            "command.profile.gz",
            Url::from_file_path(path).unwrap().to_string(),
        );

        import(&logs, &fixture.id, &fixture.dir).unwrap();

        assert_eq!(saved_json(&fixture.dir), fixture.json());
    }

    #[test]
    fn mismatched_invocation_does_not_publish_or_leave_partial_file() {
        let fixture = Fixture::new();
        let logs = fixture.source("command.profile.gz", &gzip(&fixture.json()));

        assert!(import(&logs, &uuid::Uuid::new_v4().to_string(), &fixture.dir).is_err());

        assert!(!fixture.dir.exists(PROFILE).unwrap());
        assert!(!fixture.dir.exists(TEMP).unwrap());
    }

    #[test]
    fn corrupted_gzip_does_not_publish() {
        let fixture = Fixture::new();
        let mut bytes = gzip(&fixture.json());
        bytes.pop();
        let logs = fixture.source("command.profile.gz", &bytes);

        assert!(import(&logs, &fixture.id, &fixture.dir).is_err());
        assert!(!fixture.dir.exists(PROFILE).unwrap());
        assert!(!fixture.dir.exists(TEMP).unwrap());
    }

    #[test]
    fn ignores_unrelated_build_tool_logs() {
        let fixture = Fixture::new();
        let logs = fixture.source("other.log", b"not a profile");

        import(&logs, &fixture.id, &fixture.dir).unwrap();

        assert!(!fixture.dir.exists(PROFILE).unwrap());
    }

    #[test]
    fn refuses_remote_uris() {
        assert!(open_source("https://example.com/profile.gz").is_err());
        assert!(open_source("file://example.com/profile.gz").is_err());
        assert!(open_source("file:///tmp/profile.gz?query").is_err());
        assert!(open_source("file:///tmp/profile.gz#fragment").is_err());
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
    fn bounds_expansion_and_requires_complete_json() {
        let fixture = Fixture::new();
        let bytes = fixture.json();
        assert!(validate(gzip(&bytes).as_slice(), &fixture.id, bytes.len()).is_ok());
        assert!(validate(gzip(&bytes).as_slice(), &fixture.id, bytes.len() - 1).is_err());
        let mut trailing = gzip(&bytes);
        trailing.extend(gzip(b"junk"));
        assert!(validate(trailing.as_slice(), &fixture.id, EXPANDED_LIMIT).is_err());
    }

    #[test]
    fn bounded_copy_stops_after_limit_probe() {
        let mut output = Vec::new();
        assert!(copy_bounded(b"123456789".as_slice(), &mut output, 4).is_err());
        assert_eq!(output, b"12345");
    }
}
