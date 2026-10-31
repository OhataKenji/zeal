//! Connection-local event writer. SQL holds only invocation facts and the
//! current receiving attempt; acknowledgements follow EOF, flush and SQL.
use super::{ErrorKind, Store, StoreError, database, fail, id};
use crate::protocol::{bep, bes};
use bep::build_event::Payload;
use bep::build_event_id::Id;
use prost::Message;
use rusqlite::{OptionalExtension, params};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub const FRAME_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Default)]
struct Facts {
    workspace: Option<PathBuf>,
    cmd: Option<String>,
    target: Option<String>,
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
    writer: Option<BufWriter<File>>,
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
        let run_id = id(&stream.invocation_id)?;
        let attempt = {
            let mut store = Store::open(data)?;
            let tx = store
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute(
                "INSERT INTO run(id) VALUES(?1) ON CONFLICT(id) DO NOTHING",
                [&run_id],
            )?;
            let (previous, complete): (i64, bool) = tx.query_row(
                "SELECT attempt,received_complete FROM run WHERE id=?1",
                [&run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let next = if complete {
                None
            } else {
                let next = previous
                    .checked_add(1)
                    .ok_or_else(|| fail(ErrorKind::Capacity))?;
                tx.execute(
                    "UPDATE run SET attempt=?2 WHERE id=?1 AND received_complete=0",
                    params![run_id, next],
                )?;
                Some(next)
            };
            tx.commit()?;
            next
        };
        // No Store or SQL transaction survives into file writes.
        let writer = if let Some(attempt) = attempt {
            let root = super::securefs::Dir::root(data)?;
            let dir = root
                .child("runs", true)?
                .child(&run_id, true)?
                .child("attempt", true)?
                .create_child(&attempt.to_string())?;
            Some(BufWriter::with_capacity(
                64 * 1024,
                dir.file("bep.binpb", true)?,
            ))
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

    pub fn append(&mut self, ordered: &bes::OrderedBuildEvent) -> Result<(), StoreError> {
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
        if let Some(any) = &wrapper.bazel_event {
            if self.last_message {
                return Err(fail(ErrorKind::Conflict));
            }
            if self.stream.component != 3
                || !any.type_url.ends_with("/build_event_stream.BuildEvent")
                || any.value.len() > FRAME_LIMIT
            {
                return Err(fail(ErrorKind::Invalid));
            }
            let event = bep::BuildEvent::decode(any.value.as_slice())
                .map_err(|_| fail(ErrorKind::Invalid))?;
            let started = matches!(event.payload, Some(Payload::Started(_)));
            self.observe(&event)?;
            if let Some(writer) = &mut self.writer {
                // Keep the decoded BuildEvent, including Progress stdout/stderr.
                let mut bytes = Vec::with_capacity(10);
                prost::encoding::encode_varint(any.value.len() as u64, &mut bytes);
                writer.write_all(&bytes)?;
                writer.write_all(&any.value)?;
            }
            if started {
                self.save(false)?;
            }
            self.last_message = event.last_message;
        }
        self.first.get_or_insert(ordered.sequence_number);
        self.last = ordered.sequence_number;
        Ok(())
    }

    #[allow(deprecated)]
    fn observe(&mut self, event: &bep::BuildEvent) -> Result<(), StoreError> {
        if let Some(Payload::Started(started)) = &event.payload {
            if id(&started.uuid)? != id(&self.stream.invocation_id)? {
                return Err(fail(ErrorKind::Conflict));
            }
            if self.facts.workspace.is_some()
                || started.workspace_directory.len() > 16 * 1024
                || started.command.len() > 64
            {
                return Err(fail(ErrorKind::Invalid));
            }
            let mut target = String::new();
            for pattern in event
                .children
                .iter()
                .filter_map(|v| v.id.as_ref())
                .filter_map(|v| match v {
                    Id::Pattern(p) | Id::PatternSkipped(p) => Some(&p.pattern),
                    _ => None,
                })
                .flatten()
            {
                let separator = usize::from(!target.is_empty());
                if target
                    .len()
                    .saturating_add(separator)
                    .saturating_add(pattern.len())
                    > 64 * 1024
                {
                    return Err(fail(ErrorKind::Invalid));
                }
                if separator != 0 {
                    target.push(' ');
                }
                target.push_str(pattern);
            }
            self.facts.workspace = Some(database::workspace_path(Path::new(
                &started.workspace_directory,
            ))?);
            self.facts.cmd = Some(started.command.clone());
            self.facts.target = Some(target);
            self.facts.started = millis(started.start_time.as_ref(), started.start_time_millis);
        }
        if let Some(Payload::Finished(finished)) = &event.payload {
            self.facts.finished =
                millis(finished.finish_time.as_ref(), finished.finish_time_millis);
            self.facts.exit = finished.exit_code.as_ref().map(|v| v.code);
        }
        Ok(())
    }

    fn save(&self, complete: bool) -> Result<(), StoreError> {
        let mut store = Store::open(&self.data)?;
        let tx = store
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let run_id = id(&self.stream.invocation_id)?;
        let existing: (Option<Vec<u8>>,Option<String>,Option<String>,Option<i64>)=tx.query_row("SELECT w.canonical_path,r.cmd,r.target_raw,r.started_at_ms FROM run r LEFT JOIN workspace w ON w.id=r.workspace_id WHERE r.id=?1",[&run_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        fn conflicts<T: PartialEq>(old: Option<&T>, new: Option<&T>) -> bool {
            old.zip(new).is_some_and(|(a, b)| a != b)
        }
        let path = self
            .facts
            .workspace
            .as_ref()
            .map(|v| v.as_os_str().as_bytes().to_vec());
        if conflicts(existing.0.as_ref(), path.as_ref())
            || conflicts(existing.1.as_ref(), self.facts.cmd.as_ref())
            || conflicts(existing.2.as_ref(), self.facts.target.as_ref())
            || conflicts(existing.3.as_ref(), self.facts.started.as_ref())
        {
            return Err(fail(ErrorKind::Conflict));
        }
        if let Some(attempt) = self.attempt {
            let current: bool = tx.query_row(
                "SELECT attempt=?2 AND received_complete=0 FROM run WHERE id=?1",
                params![run_id, attempt],
                |r| r.get(0),
            )?;
            if !current {
                return Err(fail(ErrorKind::Conflict));
            }
            let ws_id = if let Some(path) = path {
                tx.execute("INSERT INTO workspace(id,canonical_path) VALUES(?1,?2) ON CONFLICT(canonical_path) DO NOTHING",params![uuid::Uuid::new_v4().to_string(),path])?;
                tx.query_row(
                    "SELECT id FROM workspace WHERE canonical_path=?1",
                    [path],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            } else {
                None
            };
            let updated=tx.execute("UPDATE run SET workspace_id=coalesce(workspace_id,?3),cmd=coalesce(cmd,?4),target_raw=coalesce(target_raw,?5),started_at_ms=coalesce(started_at_ms,?6),finished_at_ms=?7,exit_code=?8,received_complete=?9 WHERE id=?1 AND attempt=?2 AND received_complete=0",params![run_id,attempt,ws_id,self.facts.cmd,self.facts.target,self.facts.started,self.facts.finished,self.facts.exit,complete])?;
            if updated != 1 {
                return Err(fail(ErrorKind::Conflict));
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<AckRange, StoreError> {
        if let Some(writer) = &mut self.writer {
            writer.flush()?;
        }
        self.save(self.last_message)?;
        Ok(AckRange {
            first: self.first.ok_or_else(|| fail(ErrorKind::Invalid))?,
            last: self.last,
        })
    }
    pub fn partial(mut self) -> Result<(), StoreError> {
        if let Some(writer) = &mut self.writer {
            writer.flush()?;
        }
        self.save(false)
    }
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
            self.data.join("runs").join(&self.stream.invocation_id)
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
        let file = artifact(&f.data, &f.ws, &record.id, "bep.binpb").unwrap();
        let len = file.metadata().unwrap().len();
        let parsed = crate::bep::parse_detail(file, len);
        assert_eq!(parsed.log, "errout");
        assert!(!f.dir().join("log.txt").exists());
        assert!(!f.dir().join("bep.json").exists());
    }
    #[test]
    fn completed_retry_is_noop_and_accepts_contiguous_suffix_without_rewriting_files() {
        let f = Fixture::new();
        f.complete();
        let path = f.dir().join("attempt/1/bep.binpb");
        let bytes = std::fs::read(&path).unwrap();
        let mut retry = Receipt::begin(&f.data, &f.stream).unwrap();
        retry.append(&f.ordered(2, f.finished())).unwrap();
        let ack = retry.finish().unwrap();
        assert_eq!((ack.first, ack.last), (2, 2));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert!(!f.dir().join("attempt/2").exists());
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
            .query_row("SELECT attempt,received_complete FROM run", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(state, (2, true));
    }

    #[test]
    fn flush_failure_and_database_failure_never_return_ack_or_mark_complete() {
        let f = Fixture::new();
        let mut receipt = Receipt::begin(&f.data, &f.stream).unwrap();
        let path = f.dir().join("attempt/1/bep.binpb");
        // A read-only FD deterministically fails when the buffer is flushed.
        receipt.writer = Some(BufWriter::new(File::open(path).unwrap()));
        receipt.append(&f.ordered(1, f.started())).unwrap();
        receipt.append(&f.ordered(2, f.finished())).unwrap();
        assert!(receipt.finish().is_err());
        let complete: bool = Store::open(&f.data)
            .unwrap()
            .conn
            .query_row("SELECT received_complete FROM run", [], |r| r.get(0))
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
            !std::fs::read(f.dir().join("attempt/2/bep.binpb"))
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
        let file = artifact(&f.data, &f.ws, &f.stream.invocation_id, "bep.binpb").unwrap();
        let length = file.metadata().unwrap().len();
        assert!(length > crate::bep::DISPLAY_LIMIT as u64);
        let parsed = crate::bep::parse_detail(file, length);
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
