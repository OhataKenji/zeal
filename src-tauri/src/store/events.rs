//! Connection-local event writer. SQL holds only invocation facts and the
//! current receiving attempt; acknowledgements follow EOF, flush and SQL.
use super::securefs::Dir;
use super::{ErrorKind, Store, StoreError, database, fail, id};
use crate::profile::COMPRESSED_LIMIT;
use crate::protocol::{bep, bes};
use bep::build_event::Payload;
use bep::build_event_id::Id;
use flate2::Compression;
use flate2::write::GzEncoder;
use prost::Message;
use rusqlite::{OptionalExtension, params};
use rustix::fs::{Mode, OFlags};
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use url::Url;

pub const FRAME_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Default)]
struct Facts {
    workspace: Option<PathBuf>,
    cmd: Option<String>,
    pattern: Option<String>,
    started: Option<i64>,
    finished: Option<i64>,
    exit: Option<i32>,
}

#[derive(Debug)]
pub struct AckRange {
    pub first: i64,
    pub last: i64,
}

pub struct Receipt {
    data: PathBuf,
    stream: bes::StreamId,
    attempt: Option<i64>,
    writer: Option<(Dir, GzEncoder<BufWriter<File>>)>,
    first: Option<i64>,
    last: i64,
    last_message: bool,
    facts: Facts,
}

impl Receipt {
    pub fn begin(data: &Path, stream: &bes::StreamId) -> Result<Self, StoreError> {
        if stream.build_id.is_empty()
            || stream.build_id.len() > 256
            || stream.invocation_id.len() > 256
            || !(1..=3).contains(&stream.component)
        {
            return Err(fail(ErrorKind::Invalid));
        }
        let invocation_id = id(&stream.invocation_id)?;
        let attempt = reserve_attempt(data, &invocation_id)?;
        // No Store or SQL transaction survives into file writes.
        let writer = if let Some(attempt) = attempt {
            let root = Dir::root(data)?;
            let dir = root
                .child("invocations", true)?
                .child(&invocation_id, true)?
                .child("attempt", true)?
                .create_child(&attempt.to_string())?;
            let writer = GzEncoder::new(
                BufWriter::with_capacity(64 * 1024, dir.file("bep.binpb.gz", true)?),
                Compression::default(),
            );
            Some((dir, writer))
        } else {
            None
        };
        Ok(Self {
            data: data.into(),
            stream: stream.clone(),
            attempt,
            writer,
            first: None,
            last: 0,
            last_message: false,
            facts: Facts::default(),
        })
    }

    /// Reports whether Started was committed to the history database.
    pub fn append(&mut self, ordered: &bes::OrderedBuildEvent) -> Result<bool, StoreError> {
        if ordered.stream_id.as_ref() != Some(&self.stream)
            || ordered.sequence_number < 1
            || ordered.encoded_len() > FRAME_LIMIT
            || self.first.is_some() && self.last.checked_add(1) != Some(ordered.sequence_number)
            || self.first.is_none() && self.attempt.is_some() && ordered.sequence_number != 1
        {
            return Err(fail(ErrorKind::Conflict));
        }
        let wrapper = ordered
            .event
            .as_ref()
            .ok_or_else(|| fail(ErrorKind::Invalid))?;
        let changed = if let Some(any) = &wrapper.bazel_event {
            self.append_bazel_event(any)?
        } else {
            false
        };
        self.first.get_or_insert(ordered.sequence_number);
        self.last = ordered.sequence_number;
        Ok(changed)
    }

    fn append_bazel_event(&mut self, any: &prost_types::Any) -> Result<bool, StoreError> {
        if self.last_message {
            return Err(fail(ErrorKind::Conflict));
        }
        if self.stream.component != 3
            || !any.type_url.ends_with("/build_event_stream.BuildEvent")
            || any.value.len() > FRAME_LIMIT
        {
            return Err(fail(ErrorKind::Invalid));
        }
        let event =
            bep::BuildEvent::decode(any.value.as_slice()).map_err(|_| fail(ErrorKind::Invalid))?;
        self.observe(&event)?;
        let profile_uri = match &event.payload {
            Some(Payload::BuildToolLogs(logs)) => logs.log.iter().find_map(|log| match &log.file {
                Some(bep::file::File::Uri(uri)) if log.name == "command.profile.gz" => Some(uri),
                _ => None,
            }),
            _ => None,
        };
        if let Some((dir, writer)) = &mut self.writer {
            // Keep the decoded BuildEvent, including Progress stdout/stderr.
            let mut bytes = Vec::with_capacity(10);
            prost::encoding::encode_varint(any.value.len() as u64, &mut bytes);
            writer.write_all(&bytes)?;
            writer.write_all(&any.value)?;
            // An unavailable profile must not fail the build receipt.
            profile_uri
                .map_or(Ok(()), |uri| copy_profile(uri, dir))
                .unwrap_or_else(|_| log::warn!("Could not copy invocation profile"));
        }
        let changed = matches!(event.payload, Some(Payload::Started(_)));
        if changed {
            self.save(false)?;
        }
        self.last_message = event.last_message;
        Ok(changed)
    }

    #[allow(deprecated)]
    fn observe(&mut self, event: &bep::BuildEvent) -> Result<(), StoreError> {
        match &event.payload {
            Some(Payload::Started(started)) => self.observe_started(started, &event.children)?,
            Some(Payload::Finished(finished)) => {
                self.facts.finished =
                    millis(finished.finish_time.as_ref(), finished.finish_time_millis);
                self.facts.exit = finished.exit_code.as_ref().map(|v| v.code);
            }
            _ => {}
        }
        Ok(())
    }

    #[allow(deprecated)]
    fn observe_started(
        &mut self,
        started: &bep::BuildStarted,
        children: &[bep::BuildEventId],
    ) -> Result<(), StoreError> {
        if id(&started.uuid)? != id(&self.stream.invocation_id)? {
            return Err(fail(ErrorKind::Conflict));
        }
        if self.facts.workspace.is_some()
            || started.workspace_directory.len() > 16 * 1024
            || started.command.len() > 64
        {
            return Err(fail(ErrorKind::Invalid));
        }
        let pattern = target_patterns(children)?;
        self.facts.workspace = Some(database::workspace_path(Path::new(
            &started.workspace_directory,
        ))?);
        self.facts.cmd = Some(started.command.clone());
        self.facts.pattern = Some(pattern);
        self.facts.started = millis(started.start_time.as_ref(), started.start_time_millis);
        Ok(())
    }

    fn save(&self, complete: bool) -> Result<(), StoreError> {
        struct StoredFacts {
            workspace: Option<Vec<u8>>,
            cmd: Option<String>,
            pattern: Option<String>,
            started: Option<i64>,
            finished: Option<i64>,
            exit: Option<i32>,
        }

        let mut store = Store::open(&self.data)?;
        let tx = store
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let invocation_id = id(&self.stream.invocation_id)?;
        let existing = tx.query_row(
            "SELECT w.canonical_path,r.cmd,r.target_pattern,r.started_at_ms,r.finished_at_ms,r.exit_code FROM invocation r LEFT JOIN workspace w ON w.id=r.workspace_id WHERE r.id=?1",
            [&invocation_id],
            |r| {
                Ok(StoredFacts {
                    workspace: r.get(0)?,
                    cmd: r.get(1)?,
                    pattern: r.get(2)?,
                    started: r.get(3)?,
                    finished: r.get(4)?,
                    exit: r.get(5)?,
                })
            },
        )?;
        fn conflicts<T: PartialEq>(old: Option<&T>, new: Option<&T>) -> bool {
            old.zip(new).is_some_and(|(a, b)| a != b)
        }
        let path = self
            .facts
            .workspace
            .as_ref()
            .map(|v| v.as_os_str().as_bytes().to_vec());
        // Reject contradictory reports; an absent value is not a contradiction.
        if conflicts(existing.workspace.as_ref(), path.as_ref())
            || conflicts(existing.cmd.as_ref(), self.facts.cmd.as_ref())
            || conflicts(existing.pattern.as_ref(), self.facts.pattern.as_ref())
            || conflicts(existing.started.as_ref(), self.facts.started.as_ref())
            || conflicts(existing.finished.as_ref(), self.facts.finished.as_ref())
            || conflicts(existing.exit.as_ref(), self.facts.exit.as_ref())
        {
            return Err(fail(ErrorKind::Conflict));
        }
        if let Some(attempt) = self.attempt {
            self.save_attempt(&tx, &invocation_id, attempt, path.as_deref(), complete)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn save_attempt(
        &self,
        tx: &rusqlite::Transaction<'_>,
        invocation_id: &str,
        attempt: i64,
        path: Option<&[u8]>,
        complete: bool,
    ) -> Result<(), StoreError> {
        let current: bool = tx.query_row(
            "SELECT attempt=?2 AND received_complete=0 FROM invocation WHERE id=?1",
            params![invocation_id, attempt],
            |r| r.get(0),
        )?;
        if !current {
            return Err(fail(ErrorKind::Conflict));
        }
        let ws_id = save_workspace(tx, path)?;
        // Missing retry facts must not erase facts already reported for this
        // invocation.
        let updated = tx.execute(
            "UPDATE invocation SET workspace_id=coalesce(workspace_id,?3),cmd=coalesce(cmd,?4),target_pattern=coalesce(target_pattern,?5),started_at_ms=coalesce(started_at_ms,?6),finished_at_ms=coalesce(finished_at_ms,?7),exit_code=coalesce(exit_code,?8),received_complete=?9 WHERE id=?1 AND attempt=?2 AND received_complete=0",
            params![invocation_id,attempt,ws_id,self.facts.cmd,self.facts.pattern,self.facts.started,self.facts.finished,self.facts.exit,complete],
        )?;
        if updated != 1 {
            return Err(fail(ErrorKind::Conflict));
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<AckRange, StoreError> {
        self.finish_writer()?;
        self.save(self.last_message)?;
        Ok(AckRange {
            first: self.first.ok_or_else(|| fail(ErrorKind::Invalid))?,
            last: self.last,
        })
    }
    pub fn partial(mut self) -> Result<(), StoreError> {
        self.finish_writer()?;
        self.save(false)
    }

    fn finish_writer(&mut self) -> Result<(), StoreError> {
        if let Some((_, writer)) = self.writer.take() {
            writer.finish()?.flush()?;
        }
        Ok(())
    }
}

/// Copy the local gzip referenced by BEP; readers wait for receipt completion.
fn copy_profile(uri: &str, dir: &Dir) -> io::Result<()> {
    let path = Url::parse(uri)
        .ok()
        .filter(|url| url.scheme() == "file")
        .and_then(|url| url.to_file_path().ok())
        .ok_or_else(|| io::Error::other("profile must reference a local file"))?;
    // Allow profile symlinks, then check and read the same FD.
    // NONBLOCK prevents a FIFO source from hanging on open.
    let source = File::from(rustix::fs::open(
        &path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )?);
    let metadata = source.metadata()?;
    if !metadata.is_file() || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(io::Error::other("profile must be an owned regular file"));
    }
    let mut copy = dir.file("profile.json.gz", true)?;
    let size = io::copy(&mut source.take(COMPRESSED_LIMIT as u64 + 1), &mut copy)?;
    if size > COMPRESSED_LIMIT as u64 {
        return Err(io::Error::other("profile exceeds byte limit"));
    }
    Ok(())
}

/// Reserve a new attempt before opening its files. Completed receipts are replayed read-only.
fn reserve_attempt(data: &Path, invocation_id: &str) -> Result<Option<i64>, StoreError> {
    let mut store = Store::open(data)?;
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO invocation(id) VALUES(?1) ON CONFLICT(id) DO NOTHING",
        [invocation_id],
    )?;
    let (previous, complete): (i64, bool) = tx.query_row(
        "SELECT attempt,received_complete FROM invocation WHERE id=?1",
        [invocation_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if complete {
        tx.commit()?;
        return Ok(None);
    }
    let next = previous
        .checked_add(1)
        .ok_or_else(|| fail(ErrorKind::Capacity))?;
    tx.execute(
        "UPDATE invocation SET attempt=?2 WHERE id=?1 AND received_complete=0",
        params![invocation_id, next],
    )?;
    tx.commit()?;
    Ok(Some(next))
}

fn target_patterns(children: &[bep::BuildEventId]) -> Result<String, StoreError> {
    let patterns = children
        .iter()
        .filter_map(|child| match child.id.as_ref() {
            Some(Id::Pattern(pattern) | Id::PatternSkipped(pattern)) => Some(&pattern.pattern),
            _ => None,
        })
        .flatten();
    let mut joined = String::new();
    for pattern in patterns {
        let separator = usize::from(!joined.is_empty());
        if joined
            .len()
            .saturating_add(separator)
            .saturating_add(pattern.len())
            > 64 * 1024
        {
            return Err(fail(ErrorKind::Invalid));
        }
        if separator != 0 {
            joined.push(' ');
        }
        joined.push_str(pattern);
    }
    Ok(joined)
}

fn save_workspace(
    tx: &rusqlite::Transaction<'_>,
    path: Option<&[u8]>,
) -> Result<Option<String>, StoreError> {
    let Some(path) = path else {
        return Ok(None);
    };
    tx.execute(
        "INSERT INTO workspace(id,canonical_path) VALUES(?1,?2) ON CONFLICT(canonical_path) DO NOTHING",
        params![uuid::Uuid::new_v4().to_string(), path],
    )?;
    Ok(tx
        .query_row(
            "SELECT id FROM workspace WHERE canonical_path=?1",
            [path],
            |r| r.get(0),
        )
        .optional()?)
}

fn millis(time: Option<&prost_types::Timestamp>, fallback: i64) -> Option<i64> {
    time.map(|v| {
        v.seconds
            .saturating_mul(1000)
            .saturating_add(i64::from(v.nanos) / 1_000_000)
            .max(0)
    })
    .or_else(|| (fallback > 0).then_some(fallback))
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    pub struct Fixture {
        pub root: PathBuf,
        pub data: PathBuf,
        pub ws: PathBuf,
        pub stream: bes::StreamId,
    }
    impl Fixture {
        pub fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-receipt-{}", uuid::Uuid::new_v4()));
            let ws = root.join("workspace");
            std::fs::create_dir_all(&ws).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            Self {
                data: root.join("data"),
                root,
                ws,
                stream: bes::StreamId {
                    build_id: id.clone(),
                    invocation_id: id,
                    component: 3,
                },
            }
        }
        pub fn ordered(&self, sequence: i64, event: bep::BuildEvent) -> bes::OrderedBuildEvent {
            bes::OrderedBuildEvent {
                stream_id: Some(self.stream.clone()),
                sequence_number: sequence,
                event: Some(bes::BuildEvent {
                    bazel_event: Some(prost_types::Any {
                        type_url: "type.googleapis.com/build_event_stream.BuildEvent".into(),
                        value: event.encode_to_vec(),
                    }),
                    ..Default::default()
                }),
            }
        }
        #[allow(deprecated)]
        pub fn started(&self) -> bep::BuildEvent {
            bep::BuildEvent {
                payload: Some(Payload::Started(bep::BuildStarted {
                    uuid: self.stream.invocation_id.clone(),
                    workspace_directory: self.ws.to_string_lossy().into_owned(),
                    command: "build".into(),
                    start_time_millis: 1000,
                    ..Default::default()
                })),
                children: vec![bep::BuildEventId {
                    id: Some(Id::Pattern(bep::build_event_id::PatternExpandedId {
                        pattern: vec!["//:target".into()],
                    })),
                }],
                ..Default::default()
            }
        }
        #[allow(deprecated)]
        pub fn finished(&self) -> bep::BuildEvent {
            bep::BuildEvent {
                payload: Some(Payload::Finished(bep::BuildFinished {
                    finish_time_millis: 2000,
                    exit_code: Some(bep::build_finished::ExitCode {
                        code: 0,
                        ..Default::default()
                    }),
                    ..Default::default()
                })),
                last_message: true,
                ..Default::default()
            }
        }
        pub fn complete(&self) {
            let mut receipt = Receipt::begin(&self.data, &self.stream).unwrap();
            receipt.append(&self.ordered(1, self.started())).unwrap();
            receipt.append(&self.ordered(2, self.finished())).unwrap();
            receipt.finish().unwrap();
        }
        pub fn dir(&self) -> PathBuf {
            self.data
                .join("invocations")
                .join(&self.stream.invocation_id)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::Fixture;
    use super::*;
    use crate::store::{artifact, list, read_record};

    #[test]
    fn refuses_nonlocal_profile_uris() {
        let fixture = Fixture::new();
        let dir = Dir::root(&fixture.root.join("attempt")).unwrap();
        assert!(copy_profile("https://example.com/profile.gz", &dir).is_err());
        assert!(copy_profile("bytestream://example.com/instance/blobs/hash/123", &dir).is_err());
        assert!(copy_profile("file://example.com/profile.gz", &dir).is_err());

        let source = fixture.root.join("profile.gz");
        std::fs::write(&source, b"profile").unwrap();
        let file_uri = Url::from_file_path(source).unwrap();
        let bytestream_uri = file_uri.as_str().replacen("file:", "bytestream:", 1);
        assert!(copy_profile(&bytestream_uri, &dir).is_err());
        let https_uri = format!("https://localhost{}", file_uri.path());
        assert!(copy_profile(&https_uri, &dir).is_err());
        assert!(!dir.exists("profile.json.gz").unwrap());
    }

    #[test]
    fn refuses_profile_fifo_without_waiting_for_writer() {
        let fixture = Fixture::new();
        let path = fixture.root.join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let dir = Dir::root(&fixture.root.join("attempt")).unwrap();
        assert!(copy_profile(Url::from_file_path(path).unwrap().as_str(), &dir).is_err());
    }

    #[test]
    fn oversized_profile_copy_stops_at_the_limit_probe() {
        let fixture = Fixture::new();
        let source = fixture.root.join("source");
        File::create(&source)
            .unwrap()
            .set_len(COMPRESSED_LIMIT as u64 + 2)
            .unwrap();
        let dir = Dir::root(&fixture.root.join("attempt")).unwrap();
        assert!(copy_profile(Url::from_file_path(source).unwrap().as_str(), &dir).is_err());
        assert_eq!(
            dir.artifact("profile.json.gz")
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            COMPRESSED_LIMIT as u64 + 1
        );
    }

    fn profile_event(path: &Path) -> bep::BuildEvent {
        bep::BuildEvent {
            payload: Some(Payload::BuildToolLogs(bep::BuildToolLogs {
                log: vec![bep::File {
                    name: "command.profile.gz".into(),
                    file: Some(bep::file::File::Uri(
                        url::Url::from_file_path(path).unwrap().into(),
                    )),
                    ..Default::default()
                }],
            })),
            last_message: true,
            ..Default::default()
        }
    }

    fn receipt_with_result(f: &Fixture) -> Receipt {
        let mut receipt = Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        let mut finished = f.finished();
        finished.last_message = false;
        receipt.append(&f.ordered(2, finished)).unwrap();
        receipt
    }

    #[test]
    fn profile_copy_survives_source_replacement_and_completed_replay() {
        let f = Fixture::new();
        let source = f.root.join("command.profile.gz");
        let expected = include_bytes!("../../tests/fixtures/invocation/ok/profile.json.gz");
        std::fs::write(&source, expected).unwrap();
        let mut receipt = receipt_with_result(&f);
        let event = profile_event(&source);
        receipt.append(&f.ordered(3, event.clone())).unwrap();
        assert_eq!(
            artifact(&f.data, &f.ws, &f.stream.invocation_id, "profile.json.gz")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
        receipt.finish().unwrap();
        // Later Bazel runs or completed BES replays must not change the copy.
        std::fs::write(&source, b"replaced").unwrap();
        let mut replay = Receipt::begin(&f.data, &f.stream).unwrap();
        replay.append(&f.ordered(3, event)).unwrap();
        replay.finish().unwrap();
        let mut saved =
            artifact(&f.data, &f.ws, &f.stream.invocation_id, "profile.json.gz").unwrap();
        let mut actual = Vec::new();
        std::io::Read::read_to_end(&mut saved, &mut actual).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn missing_profile_does_not_fail_the_build_receipt() {
        let f = Fixture::new();
        let mut receipt = receipt_with_result(&f);
        let event = profile_event(&f.root.join("missing.profile.gz"));
        receipt.append(&f.ordered(3, event)).unwrap();
        let ack = receipt.finish().unwrap();
        assert_eq!(ack.last, 3);
        assert_eq!(
            read_record(&f.data, &f.ws, &f.stream.invocation_id)
                .unwrap()
                .exit_code,
            Some(0)
        );
        assert!(artifact(&f.data, &f.ws, &f.stream.invocation_id, "profile.json.gz").is_err());
    }

    #[test]
    fn gzip_preserves_every_length_delimited_event_byte() {
        use std::io::Read;

        let f = Fixture::new();
        let expected = [
            f.started().encode_length_delimited_to_vec(),
            f.finished().encode_length_delimited_to_vec(),
        ]
        .concat();
        f.complete();
        let file = artifact(&f.data, &f.ws, &f.stream.invocation_id, "bep.binpb.gz").unwrap();
        let mut decoded = Vec::new();
        flate2::read::MultiGzDecoder::new(file)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn progress_before_started_is_saved_without_workspace_and_completion_is_a_separate_fact() {
        let f = Fixture::new();
        let mut receipt = Receipt::begin(&f.data, &f.stream).unwrap();
        let progress = bep::BuildEvent {
            payload: Some(Payload::Progress(bep::Progress {
                stdout: "out".into(),
                stderr: "err".into(),
            })),
            ..Default::default()
        };
        receipt.append(&f.ordered(1, progress)).unwrap();
        assert!(list(&f.data, &f.ws).unwrap().is_empty());
        receipt.append(&f.ordered(2, f.started())).unwrap();
        assert_eq!(list(&f.data, &f.ws).unwrap().len(), 1);
        receipt.append(&f.ordered(3, f.finished())).unwrap();
        let ack = receipt.finish().unwrap();
        assert_eq!((ack.first, ack.last), (1, 3));
        let record = read_record(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert_eq!(
            (record.started_at, record.finished_at, record.exit_code),
            (Some(1000), Some(2000), Some(0))
        );
        let file = artifact(&f.data, &f.ws, &record.id, "bep.binpb.gz").unwrap();
        let len = file.metadata().unwrap().len();
        let parsed = crate::bep::parse_gzip(file, len);
        assert_eq!(parsed.log, "errout");
        assert!(!f.dir().join("log.txt").exists());
        assert!(!f.dir().join("bep.json").exists());
    }
    #[test]
    fn completed_retry_is_noop_and_accepts_contiguous_suffix_without_rewriting_files() {
        let f = Fixture::new();
        f.complete();
        let path = f.dir().join("attempt/1/bep.binpb.gz");
        let bytes = std::fs::read(&path).unwrap();
        let mut retry = Receipt::begin(&f.data, &f.stream).unwrap();
        retry.append(&f.ordered(2, f.finished())).unwrap();
        let ack = retry.finish().unwrap();
        assert_eq!((ack.first, ack.last), (2, 2));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert!(!f.dir().join("attempt/2").exists());
    }

    fn finished(exit_code: i32, seconds: i64) -> bep::BuildEvent {
        bep::BuildEvent {
            payload: Some(Payload::Finished(bep::BuildFinished {
                finish_time: Some(prost_types::Timestamp { seconds, nanos: 0 }),
                exit_code: Some(bep::build_finished::ExitCode {
                    code: exit_code,
                    ..Default::default()
                }),
                ..Default::default()
            })),
            last_message: true,
            ..Default::default()
        }
    }

    fn save_partial_result(f: &Fixture, exit_code: i32) {
        let mut receipt = Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        receipt
            .append(&f.ordered(2, finished(exit_code, 2)))
            .unwrap();
        receipt.partial().unwrap();
    }

    fn assert_partial_retry_preserves_result(exit_code: i32) {
        let f = Fixture::new();
        save_partial_result(&f, exit_code);
        let known = read_record(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert_eq!(
            (known.finished_at, known.exit_code),
            (Some(2000), Some(exit_code))
        );

        let mut retry = Receipt::begin(&f.data, &f.stream).unwrap();
        retry.append(&f.ordered(1, f.started())).unwrap();
        assert_eq!(
            read_record(&f.data, &f.ws, &f.stream.invocation_id).unwrap(),
            known
        );
        retry.partial().unwrap();
        assert_eq!(
            read_record(&f.data, &f.ws, &f.stream.invocation_id).unwrap(),
            known
        );
    }

    #[test]
    fn partial_retry_preserves_known_success() {
        assert_partial_retry_preserves_result(0);
    }

    #[test]
    fn partial_retry_preserves_known_failure() {
        assert_partial_retry_preserves_result(1);
    }

    fn assert_retry_rejects_conflicting_result(exit_code: i32, seconds: i64) {
        let f = Fixture::new();
        save_partial_result(&f, 0);
        let known = read_record(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        let mut retry = Receipt::begin(&f.data, &f.stream).unwrap();
        retry.append(&f.ordered(1, f.started())).unwrap();
        retry
            .append(&f.ordered(2, finished(exit_code, seconds)))
            .unwrap();
        assert_eq!(retry.finish().unwrap_err().kind, ErrorKind::Conflict);
        assert_eq!(
            read_record(&f.data, &f.ws, &f.stream.invocation_id).unwrap(),
            known
        );
        let state: (i64, bool) = Store::open(&f.data)
            .unwrap()
            .conn
            .query_row(
                "SELECT attempt,received_complete FROM invocation",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (2, false));
    }

    #[test]
    fn retry_rejects_conflicting_exit_code() {
        assert_retry_rejects_conflicting_result(1, 2);
    }

    #[test]
    fn retry_rejects_conflicting_finish_time() {
        assert_retry_rejects_conflicting_result(0, 3);
    }

    #[test]
    fn incomplete_retry_requires_full_send_and_old_started_partial_and_finish_cannot_update_new_attempt()
     {
        let f = Fixture::new();
        let mut old = Receipt::begin(&f.data, &f.stream).unwrap();
        let mut newer = Receipt::begin(&f.data, &f.stream).unwrap();
        assert_eq!(
            old.append(&f.ordered(1, f.started())).unwrap_err().kind,
            ErrorKind::Conflict
        );
        assert_eq!(old.partial().unwrap_err().kind, ErrorKind::Conflict);
        assert_eq!(
            newer.append(&f.ordered(2, f.finished())).unwrap_err().kind,
            ErrorKind::Conflict
        );
        newer.append(&f.ordered(1, f.started())).unwrap();
        let mut latest = Receipt::begin(&f.data, &f.stream).unwrap();
        latest.append(&f.ordered(1, f.started())).unwrap();
        latest.append(&f.ordered(2, f.finished())).unwrap();
        latest.finish().unwrap();
        assert_eq!(newer.finish().unwrap_err().kind, ErrorKind::Conflict);
        assert_eq!(
            read_record(&f.data, &f.ws, &f.stream.invocation_id)
                .unwrap()
                .exit_code,
            Some(0)
        );
    }
    #[test]
    fn rejects_identity_gaps_duplicates_and_conflicting_started_even_for_completed_retry() {
        let f = Fixture::new();
        let mut receipt = Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        assert!(receipt.append(&f.ordered(1, f.started())).is_err());
        assert!(receipt.append(&f.ordered(3, f.finished())).is_err());
        let mut wrong = f.ordered(2, f.finished());
        wrong.stream_id.as_mut().unwrap().build_id = "other".into();
        assert!(receipt.append(&wrong).is_err());
        receipt.append(&f.ordered(2, f.finished())).unwrap();
        receipt.finish().unwrap();
        let mut retry = Receipt::begin(&f.data, &f.stream).unwrap();
        let mut started = f.started();
        if let Some(Payload::Started(v)) = &mut started.payload {
            v.command = "test".into();
        }
        assert_eq!(
            retry.append(&f.ordered(1, started)).unwrap_err().kind,
            ErrorKind::Conflict
        );
    }
    #[test]
    fn normal_eof_without_last_message_stays_retryable_and_build_finished_absence_is_unknown() {
        let f = Fixture::new();
        let mut receipt = Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        receipt.finish().unwrap();
        let mut next = Receipt::begin(&f.data, &f.stream).unwrap();
        let mut started = f.started();
        started.last_message = true;
        next.append(&f.ordered(1, started)).unwrap();
        next.finish().unwrap();
        let record = read_record(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert_eq!((record.finished_at, record.exit_code), (None, None));
        let store = Store::open(&f.data).unwrap();
        let state: (i64, bool) = store
            .conn
            .query_row(
                "SELECT attempt,received_complete FROM invocation",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (2, true));
    }

    #[test]
    fn flush_failure_and_database_failure_never_return_ack_or_mark_complete() {
        let f = Fixture::new();
        let mut receipt = Receipt::begin(&f.data, &f.stream).unwrap();
        let path = f.dir().join("attempt/1/bep.binpb.gz");
        // A read-only FD deterministically fails when the buffer is flushed.
        receipt.writer.as_mut().unwrap().1 = GzEncoder::new(
            BufWriter::new(File::open(path).unwrap()),
            Compression::default(),
        );
        receipt.append(&f.ordered(1, f.started())).unwrap();
        receipt.append(&f.ordered(2, f.finished())).unwrap();
        assert!(receipt.finish().is_err());
        let complete: bool = Store::open(&f.data)
            .unwrap()
            .conn
            .query_row("SELECT received_complete FROM invocation", [], |r| r.get(0))
            .unwrap();
        assert!(!complete);

        let mut retry = Receipt::begin(&f.data, &f.stream).unwrap();
        retry.append(&f.ordered(1, f.started())).unwrap();
        retry.append(&f.ordered(2, f.finished())).unwrap();
        Store::open(&f.data)
            .unwrap()
            .conn
            .pragma_update(None, "user_version", 4)
            .unwrap();
        assert_eq!(retry.finish().unwrap_err().kind, ErrorKind::NewerSchema);
        assert!(
            !std::fs::read(f.dir().join("attempt/2/bep.binpb.gz"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn legal_events_larger_than_display_limit_remain_in_the_event_file() {
        let f = Fixture::new();
        let mut receipt = Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        receipt
            .append(&f.ordered(
                2,
                bep::BuildEvent {
                    payload: Some(Payload::OptionsParsed(bep::OptionsParsed {
                        cmd_line: vec!["x".repeat(crate::bep::DISPLAY_LIMIT + 1)],
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            ))
            .unwrap();
        receipt.append(&f.ordered(3, f.finished())).unwrap();
        // The transport end marker follows the last BEP event in real Bazel.
        receipt
            .append(&bes::OrderedBuildEvent {
                stream_id: Some(f.stream.clone()),
                sequence_number: 4,
                event: Some(bes::BuildEvent::default()),
            })
            .unwrap();
        assert_eq!(receipt.finish().unwrap().last, 4);
        let file = artifact(&f.data, &f.ws, &f.stream.invocation_id, "bep.binpb.gz").unwrap();
        let length = file.metadata().unwrap().len();
        assert!(length < crate::bep::DISPLAY_LIMIT as u64);
        let parsed = crate::bep::parse_gzip(file, length);
        assert!(parsed.bep.truncated);
        assert!(parsed.bep.options.is_none());
        assert!(!parsed.invalid);
    }

    #[test]
    #[allow(deprecated)]
    fn absent_bep_times_stay_unknown_even_when_build_exit_is_known() {
        let f = Fixture::new();
        let mut receipt = Receipt::begin(&f.data, &f.stream).unwrap();
        let mut started = f.started();
        if let Some(Payload::Started(value)) = &mut started.payload {
            value.start_time_millis = 0;
        }
        let mut finished = f.finished();
        if let Some(Payload::Finished(value)) = &mut finished.payload {
            value.finish_time_millis = 0;
        }
        receipt.append(&f.ordered(1, started)).unwrap();
        receipt.append(&f.ordered(2, finished)).unwrap();
        receipt.finish().unwrap();
        let record = read_record(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert_eq!(
            (record.started_at, record.finished_at, record.exit_code),
            (None, None, Some(0))
        );
    }
}
