//! Durable observations, independent of process ownership. Acknowledgements
//! follow the transaction containing both original events and their projection.
use super::{ErrorKind, RunRecord, Store, StoreError, fail};
use crate::bep::Bep;
use crate::protocol::{bep, bes};
use bep::{build_event::Payload, build_event_id::Id};
use prost::Message;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(super) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS build_event(
        build_id TEXT NOT NULL, invocation_id TEXT NOT NULL, component INTEGER NOT NULL,
        sequence INTEGER NOT NULL, payload BLOB NOT NULL,
        PRIMARY KEY(build_id,invocation_id,component,sequence));
    CREATE INDEX IF NOT EXISTS build_event_invocation ON build_event(invocation_id);
    CREATE TABLE IF NOT EXISTS observation(
        id TEXT PRIMARY KEY, workspace TEXT NOT NULL, record TEXT NOT NULL, summary TEXT NOT NULL, complete INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS observation_workspace ON observation(workspace);
    PRAGMA user_version=2;";

pub const LOG_LIMIT: usize = 512 * 1024;
const SUMMARY_LIMIT: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub id: String,
    pub workspace: String,
    pub cmd: String,
    pub target: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub exit_code: Option<i32>,
    pub complete: bool,
    pub bep: Bep,
    pub log: String,
}

impl Observation {
    pub fn record(&self) -> RunRecord {
        RunRecord {
            id: self.id.clone(),
            workspace: self.workspace.clone(),
            cmd: self.cmd.clone(),
            target: self.target.clone(),
            started_at: self.started_at,
            finished_at: self.finished_at,
            exit_code: self.exit_code,
            external: true,
        }
    }

    /// Pure projection. The saved event bytes retain fields not shown by this UI.
    #[allow(deprecated)] // Fall back to legacy millisecond timestamps when needed.
    fn apply(&mut self, mut event: bep::BuildEvent) {
        if let Some(Payload::Started(started)) = &event.payload {
            self.workspace = started.workspace_directory.clone();
            self.cmd = started.command.clone();
            self.started_at = millis(started.start_time.as_ref(), started.start_time_millis);
            self.target = event
                .children
                .iter()
                .filter_map(|id| id.id.as_ref())
                .filter_map(|id| match id {
                    Id::Pattern(pattern) | Id::PatternSkipped(pattern) => Some(&pattern.pattern),
                    _ => None,
                })
                .flat_map(|patterns| patterns.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
        }
        if let Some(Payload::Progress(progress)) = &event.payload {
            self.log.push_str(&progress.stderr);
            self.log.push_str(&progress.stdout);
            trim_log(&mut self.log);
        }
        if let Some(Payload::Finished(finished)) = &event.payload {
            self.finished_at = Some(millis(
                finished.finish_time.as_ref(),
                finished.finish_time_millis,
            ));
            self.exit_code = finished.exit_code.as_ref().map(|exit| exit.code);
        }
        self.bep.apply_event(event.id.take(), event.payload.take());
        self.complete |= event.last_message;
    }
}

pub fn trim_log(log: &mut String) -> bool {
    if log.len() <= LOG_LIMIT {
        return false;
    }
    let mut start = log.len() - LOG_LIMIT;
    while !log.is_char_boundary(start) {
        start += 1;
    }
    log.drain(..start);
    true
}

fn millis(time: Option<&prost_types::Timestamp>, fallback: i64) -> u64 {
    time.map(|t| {
        t.seconds
            .saturating_mul(1000)
            .saturating_add(i64::from(t.nanos) / 1_000_000)
    })
    .unwrap_or(fallback)
    .max(0) as u64
}

/// Reject gaps and contradictory retries; acknowledge identical retries without
/// applying their effects twice. Workspaces come from BuildStarted, not the UI.
#[tracing::instrument(target = "zeal::operations", name = "bes.save_event", skip_all)]
pub fn save(data: &Path, ordered: &bes::OrderedBuildEvent) -> Result<(), StoreError> {
    let stream = ordered
        .stream_id
        .as_ref()
        .ok_or_else(|| fail(ErrorKind::Invalid))?;
    if stream.build_id.is_empty()
        || stream.build_id.len() > 256
        || stream.invocation_id.len() > 256
        || ordered.sequence_number < 1
        || !(1..=3).contains(&stream.component)
    {
        return Err(fail(ErrorKind::Invalid));
    }
    let wrapper = ordered
        .event
        .as_ref()
        .ok_or_else(|| fail(ErrorKind::Invalid))?;
    let event = wrapper
        .bazel_event
        .as_ref()
        .map(|any| {
            if !any.type_url.ends_with("/build_event_stream.BuildEvent") {
                return Err(fail(ErrorKind::Invalid));
            }
            bep::BuildEvent::decode(any.value.as_slice()).map_err(|_| fail(ErrorKind::Invalid))
        })
        .transpose()?;
    if event.is_some() {
        if stream.component != 3 {
            return Err(fail(ErrorKind::Invalid));
        }
        super::id(&stream.invocation_id)?;
    }
    let bytes = ordered.encode_to_vec();
    let mut store = Store::open(data)?;
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if event.is_some() {
        let existing: Option<String> = tx
            .query_row(
                "SELECT build_id FROM build_event WHERE invocation_id=?1 LIMIT 1",
                params![stream.invocation_id],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some_and(|build| build != stream.build_id) {
            return Err(fail(ErrorKind::Conflict));
        }
    }
    let previous: Option<Vec<u8>> = tx.query_row(
        "SELECT payload FROM build_event WHERE build_id=?1 AND invocation_id=?2 AND component=?3 AND sequence=?4",
        params![stream.build_id, stream.invocation_id, stream.component, ordered.sequence_number],
        |row| row.get(0)).optional()?;
    if let Some(previous) = previous {
        return if previous == bytes {
            Ok(())
        } else {
            Err(fail(ErrorKind::Conflict))
        };
    }
    let last: i64 = tx.query_row(
        "SELECT coalesce(max(sequence),0) FROM build_event WHERE build_id=?1 AND invocation_id=?2 AND component=?3",
        params![stream.build_id, stream.invocation_id, stream.component], |row| row.get(0))?;
    if ordered.sequence_number != last + 1 {
        return Err(fail(ErrorKind::Conflict));
    }
    tx.execute(
        "INSERT INTO build_event VALUES(?1,?2,?3,?4,?5)",
        params![
            stream.build_id,
            stream.invocation_id,
            stream.component,
            ordered.sequence_number,
            bytes
        ],
    )?;
    if let Some(mut event) = event {
        if let Some(Payload::Started(started)) = &mut event.payload {
            if started.uuid != stream.invocation_id {
                return Err(fail(ErrorKind::Conflict));
            }
            if !Path::new(&started.workspace_directory).is_absolute()
                || started.workspace_directory.len() > 16 * 1024
                || started.command.len() > 64
            {
                return Err(fail(ErrorKind::Invalid));
            }
            if let Ok(path) = Path::new(&started.workspace_directory).canonicalize() {
                started.workspace_directory = path.to_string_lossy().into_owned();
            }
        }
        let saved: Option<String> = tx
            .query_row(
                "SELECT summary FROM observation WHERE id=?1",
                [&stream.invocation_id],
                |row| row.get(0),
            )
            .optional()?;
        let mut observation = saved
            .as_ref()
            .map(|value| decode(value))
            .transpose()?
            .unwrap_or_else(|| Observation {
                id: stream.invocation_id.clone(),
                ..Observation::default()
            });
        if observation.complete
            || (!observation.workspace.is_empty()
                && matches!(event.payload.as_ref(), Some(Payload::Started(_))))
        {
            return Err(fail(ErrorKind::Conflict));
        }
        // Bazel may publish progress before BuildStarted. Keep those events;
        // the observation becomes workspace-visible once BuildStarted arrives.
        let targets = observation.bep.targets.len();
        let tests = observation.bep.tests.len();
        observation.apply(event);
        if observation.target.len() > 64 * 1024 {
            return Err(fail(ErrorKind::Invalid));
        }
        let mut summary =
            serde_json::to_string(&observation).map_err(|_| fail(ErrorKind::Corrupt))?;
        if summary.len() > SUMMARY_LIMIT {
            observation.bep.targets.truncate(targets);
            observation.bep.tests.truncate(tests);
            observation.bep.truncated = true;
            summary = serde_json::to_string(&observation).map_err(|_| fail(ErrorKind::Corrupt))?;
            if summary.len() > SUMMARY_LIMIT {
                return Err(fail(ErrorKind::Capacity));
            }
        }
        let record =
            serde_json::to_string(&observation.record()).map_err(|_| fail(ErrorKind::Corrupt))?;
        tx.execute("INSERT INTO observation(id,workspace,record,summary,complete) VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(id) DO UPDATE SET workspace=excluded.workspace,record=excluded.record,summary=excluded.summary,complete=excluded.complete",
            params![observation.id, observation.workspace, record, summary, observation.complete])?;
    }
    tx.commit()?;
    Ok(())
}

fn decode(value: &str) -> Result<Observation, StoreError> {
    serde_json::from_str(value).map_err(|_| fail(ErrorKind::Corrupt))
}

#[tracing::instrument(target = "zeal::operations", name = "detail.observation", skip_all)]
pub fn get(data: &Path, ws: &Path, id: &str) -> Result<Option<Observation>, StoreError> {
    read(&Store::open(data)?, &ws.canonicalize()?, id)
}

pub(super) fn read(store: &Store, ws: &Path, id: &str) -> Result<Option<Observation>, StoreError> {
    let summary: Option<String> = store
        .conn
        .query_row(
            "SELECT summary FROM observation WHERE id=?1 AND workspace=?2",
            params![super::id(id)?, ws.to_string_lossy()],
            |row| row.get(0),
        )
        .optional()?;
    summary.map(|value| decode(&value)).transpose()
}

pub(super) fn list(store: &Store, ws: &Path) -> Result<Vec<RunRecord>, StoreError> {
    store
        .conn
        .prepare("SELECT record FROM observation WHERE workspace=?1 AND complete=1")?
        .query_map([ws.to_string_lossy()], |row| row.get::<_, String>(0))?
        .map(|row| serde_json::from_str(&row?).map_err(|_| fail(ErrorKind::Corrupt)))
        .collect()
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use bep::{
        build_event::Payload,
        build_event_id::{Id, PatternExpandedId, TargetCompletedId, TestSummaryId},
        build_finished::ExitCode,
    };
    use std::{fs, path::PathBuf};

    fn payload(payload: Payload) -> bep::BuildEvent {
        bep::BuildEvent {
            payload: Some(payload),
            ..Default::default()
        }
    }

    struct Fixture {
        root: PathBuf,
        ws: PathBuf,
        data: PathBuf,
        id: String,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-events-{}", uuid::Uuid::new_v4()));
            let ws = root.join("workspace");
            fs::create_dir_all(&ws).unwrap();
            Self {
                data: root.join("data"),
                root,
                ws: ws.canonicalize().unwrap(),
                id: uuid::Uuid::new_v4().to_string(),
            }
        }
        fn event(&self, sequence: i64, event: bep::BuildEvent) -> bes::OrderedBuildEvent {
            bes::OrderedBuildEvent {
                stream_id: Some(bes::StreamId {
                    build_id: "build".into(),
                    invocation_id: self.id.clone(),
                    component: 3,
                }),
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
        fn start(&self) -> bes::OrderedBuildEvent {
            self.event(
                1,
                bep::BuildEvent {
                    payload: Some(Payload::Started(bep::BuildStarted {
                        uuid: self.id.clone(),
                        command: "run".into(),
                        workspace_directory: self.ws.to_string_lossy().into_owned(),
                        start_time_millis: 123,
                        ..Default::default()
                    })),
                    children: vec![bep::BuildEventId {
                        id: Some(Id::Pattern(PatternExpandedId {
                            pattern: vec!["//:double".into()],
                        })),
                    }],
                    ..Default::default()
                },
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn progress_before_build_started_is_retained_and_bound_to_one_stream() {
        let f = Fixture::new();
        let progress = f.event(
            1,
            payload(Payload::Progress(bep::Progress {
                stdout: "early output".into(),
                ..Default::default()
            })),
        );
        save(&f.data, &progress).unwrap();
        assert!(get(&f.data, &f.ws, &f.id).unwrap().is_none());
        let mut start = f.start();
        start.sequence_number = 2;
        save(&f.data, &start).unwrap();
        assert_eq!(
            get(&f.data, &f.ws, &f.id).unwrap().unwrap().log,
            "early output"
        );
        start.sequence_number = 3;
        assert_eq!(save(&f.data, &start).unwrap_err().kind, ErrorKind::Conflict);
    }

    #[test]
    fn bes_and_file_paths_share_bep_projection_and_saved_history() {
        let f = Fixture::new();
        save(&f.data, &f.start()).unwrap();
        let events = [
            payload(Payload::OptionsParsed(bep::OptionsParsed {
                startup_options: vec!["--batch".into()],
                explicit_startup_options: vec![],
                cmd_line: vec!["--jobs=8".into(), "--color=yes".into()],
                explicit_cmd_line: vec!["--jobs=8".into()],
                ..Default::default()
            })),
            bep::BuildEvent {
                id: Some(bep::BuildEventId {
                    id: Some(Id::TargetCompleted(TargetCompletedId {
                        label: "//pkg:target".into(),
                        ..Default::default()
                    })),
                }),
                payload: Some(Payload::Completed(bep::TargetComplete {
                    success: true,
                    ..Default::default()
                })),
                ..Default::default()
            },
            bep::BuildEvent {
                id: Some(bep::BuildEventId {
                    id: Some(Id::TestSummary(TestSummaryId {
                        label: "//pkg:test".into(),
                        ..Default::default()
                    })),
                }),
                payload: Some(Payload::TestSummary(bep::TestSummary {
                    overall_status: 1,
                    ..Default::default()
                })),
                ..Default::default()
            },
        ];
        let mut file_stream = Vec::new();
        for (index, event) in events.into_iter().enumerate() {
            event.encode_length_delimited(&mut file_stream).unwrap();
            save(&f.data, &f.event(index as i64 + 2, event)).unwrap();
        }
        let observed = get(&f.data, &f.ws, &f.id).unwrap().unwrap();
        assert_eq!(observed.bep, crate::bep::parse(file_stream.as_slice()));
        assert_eq!(observed.bep.options.as_ref().unwrap().cmd_line.len(), 2);
        assert_eq!(observed.bep.targets[0].label, "//pkg:target");
        assert_eq!(observed.bep.tests[0].label, "//pkg:test");
        assert_eq!(
            get(&f.data, &f.ws, &f.id).unwrap().unwrap().bep,
            observed.bep
        );
    }

    #[test]
    fn another_component_or_build_cannot_replace_a_completed_invocation() {
        let f = Fixture::new();
        save(&f.data, &f.start()).unwrap();
        let finished = f.event(2, {
            let mut event = payload(Payload::Finished(bep::BuildFinished {
                exit_code: Some(ExitCode {
                    code: 1,
                    ..Default::default()
                }),
                ..Default::default()
            }));
            event.last_message = true;
            event
        });
        save(&f.data, &finished).unwrap();
        let mut forged = finished.clone();
        forged.sequence_number = 1;
        forged.stream_id.as_mut().unwrap().component = 1;
        assert_eq!(save(&f.data, &forged).unwrap_err().kind, ErrorKind::Invalid);
        forged.stream_id.as_mut().unwrap().component = 3;
        forged.stream_id.as_mut().unwrap().build_id = "different".into();
        assert_eq!(
            save(&f.data, &forged).unwrap_err().kind,
            ErrorKind::Conflict
        );
        let mut after_complete = finished.clone();
        after_complete.sequence_number = 3;
        assert_eq!(
            save(&f.data, &after_complete).unwrap_err().kind,
            ErrorKind::Conflict
        );
        save(&f.data, &finished).unwrap(); // The original retry is still valid.
        assert_eq!(
            get(&f.data, &f.ws, &f.id).unwrap().unwrap().exit_code,
            Some(1)
        );
    }

    #[test]
    fn result_projection_is_bounded_without_losing_original_events() {
        let f = Fixture::new();
        save(&f.data, &f.start()).unwrap();
        let target = bep::BuildEvent {
            id: Some(bep::BuildEventId {
                id: Some(Id::TargetCompleted(TargetCompletedId {
                    label: "x".repeat(5 * 1024 * 1024),
                    ..Default::default()
                })),
            }),
            payload: Some(Payload::Completed(bep::TargetComplete {
                success: true,
                ..Default::default()
            })),
            ..Default::default()
        };
        save(&f.data, &f.event(2, target.clone())).unwrap();
        save(&f.data, &f.event(3, target)).unwrap();
        let observed = get(&f.data, &f.ws, &f.id).unwrap().unwrap();
        assert_eq!(observed.bep.targets.len(), 1);
        assert!(observed.bep.truncated);
        assert!(serde_json::to_vec(&observed).unwrap().len() <= SUMMARY_LIMIT);
        let store = Store::open(&f.data).unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM build_event", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }

    #[test]
    fn oversized_generated_policy_fails_with_capacity() {
        use crate::protocol::generated::blaze::invocation_policy as policy;

        let f = Fixture::new();
        save(&f.data, &f.start()).unwrap();
        let options = bep::OptionsParsed {
            invocation_policy: Some(policy::InvocationPolicy {
                flag_policies: vec![policy::FlagPolicy {
                    flag_name: Some("define".into()),
                    operation: Some(policy::flag_policy::Operation::SetValue(policy::SetValue {
                        flag_value: vec!["x".repeat(SUMMARY_LIMIT + 64)],
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let event = f.event(2, payload(Payload::OptionsParsed(options)));

        assert_eq!(save(&f.data, &event).unwrap_err().kind, ErrorKind::Capacity);
        assert_eq!(
            get(&f.data, &f.ws, &f.id).unwrap().unwrap().bep.options,
            None
        );
        let store = Store::open(&f.data).unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM build_event", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn external_events_survive_reopen_and_retries_do_not_duplicate_output() {
        let f = Fixture::new();
        save(&f.data, &f.start()).unwrap();
        let progress = f.event(
            2,
            payload(Payload::Progress(bep::Progress {
                stdout: "hello\n".into(),
                stderr: String::new(),
            })),
        );
        save(&f.data, &progress).unwrap();
        save(&f.data, &progress).unwrap();
        let observation = get(&f.data, &f.ws, &f.id).unwrap().unwrap();
        assert_eq!(observation.log, "hello\n");
        assert!(!observation.complete);
        assert!(super::super::list(&f.data, &f.ws).unwrap().is_empty());
        // BuildFinished can precede final test/target results: publish only at last_message.
        save(
            &f.data,
            &f.event(3, payload(Payload::Finished(bep::BuildFinished::default()))),
        )
        .unwrap();
        assert!(super::super::list(&f.data, &f.ws).unwrap().is_empty());
        save(
            &f.data,
            &f.event(
                4,
                bep::BuildEvent {
                    last_message: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        assert_eq!(super::super::list(&f.data, &f.ws).unwrap().len(), 1);
        assert!(get(&f.data, &f.root, &f.id).unwrap().is_none());
    }

    #[test]
    fn gaps_and_conflicting_retries_are_not_acknowledged() {
        let f = Fixture::new();
        save(&f.data, &f.start()).unwrap();
        let gap = f.event(3, bep::BuildEvent::default());
        assert_eq!(save(&f.data, &gap).unwrap_err().kind, ErrorKind::Conflict);
        let conflicting = f.event(1, bep::BuildEvent::default());
        assert_eq!(
            save(&f.data, &conflicting).unwrap_err().kind,
            ErrorKind::Conflict
        );
        assert!(!get(&f.data, &f.ws, &f.id).unwrap().unwrap().complete);
    }

    #[test]
    fn build_completion_does_not_finish_an_owned_program() {
        let mut f = Fixture::new();
        let (owned, _) = super::super::create(&f.data, &f.ws, "run", "//:double").unwrap();
        f.id = owned.id.clone();
        save(&f.data, &f.start()).unwrap();
        save(
            &f.data,
            &f.event(2, {
                let mut event = payload(Payload::Finished(bep::BuildFinished {
                    exit_code: Some(ExitCode {
                        code: 0,
                        ..Default::default()
                    }),
                    finish_time_millis: 456,
                    ..Default::default()
                }));
                event.last_message = true;
                event
            }),
        )
        .unwrap();
        assert_eq!(
            super::super::list(&f.data, &f.ws).unwrap(),
            vec![owned.clone()]
        );
        let observed = get(&f.data, &f.ws, &f.id).unwrap().unwrap();
        assert_eq!(observed.exit_code, Some(0));
        assert!(observed.complete);
        assert_eq!(
            super::super::read_record(&f.data, &f.ws, &f.id)
                .unwrap()
                .exit_code,
            None
        );
    }

    #[test]
    fn version_one_database_migrates_without_losing_history() {
        let f = Fixture::new();
        let (owned, _) = super::super::create(&f.data, &f.ws, "build", "//:app").unwrap();
        let store = Store::open(&f.data).unwrap();
        store
            .conn
            .execute_batch("DROP TABLE build_event; DROP TABLE observation; PRAGMA user_version=1;")
            .unwrap();
        drop(store);
        save(&f.data, &f.start()).unwrap();
        assert_eq!(
            super::super::read_record(&f.data, &f.ws, &owned.id).unwrap(),
            owned
        );
        assert_eq!(super::super::list(&f.data, &f.ws).unwrap().len(), 1);
    }

    #[test]
    fn log_limit_preserves_unicode_boundaries() {
        let mut log = "日".repeat(LOG_LIMIT);
        assert!(trim_log(&mut log));
        assert!(log.len() <= LOG_LIMIT);
        assert!(log.chars().all(|ch| ch == '日'));
    }
}
