//! Read-only projections of completed GUI runs and terminal build observations.
//! Artifacts and live runner state never participate in counting or filtering.
use super::{ErrorKind, Store, StoreError};
use rusqlite::{Connection, named_params, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Filter {
    pub start: Option<String>,
    pub end: Option<String>,
    pub cmd: Option<String>,
    pub outcome: Option<String>,
    /// Exact `target_raw` pattern match. Restricts the daily series only; the
    /// target ranking always reflects every pattern in the filtered period.
    pub target: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub day: String,
    pub count: i64,
    pub ok: i64,
    pub failed: i64,
    pub interrupted: i64,
    /// Runs carrying a recorded finish time; the denominator of the
    /// duration series. `count - timed` rows contribute nothing to it.
    pub timed: i64,
    /// Wall-clock total of runs with a recorded finish time.
    pub total_ms: i64,
    /// Nearest-rank percentiles over the day's recorded durations.
    pub p50_ms: i64,
    pub p95_ms: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetDuration {
    pub target: String,
    pub runs: i64,
    pub total_ms: i64,
    pub p50_ms: i64,
    pub p95_ms: i64,
}
#[derive(Debug, Serialize)]
pub struct Analytics {
    pub filter: Filter,
    pub today: String,
    pub days: Vec<Day>,
    pub targets: Vec<TargetDuration>,
}

// Both sources use the same local-date and result rules after workspace
// selection and GUI precedence have been resolved.
const MATCH: &str = "started_at_ms >= :start AND started_at_ms < :end
    AND exit_code IS NOT NULL AND cmd IN ('build','test')
    AND (:cmd IS NULL OR cmd=:cmd)
    AND (:outcome IS NULL OR
        CASE WHEN exit_code=0 THEN 'ok' WHEN exit_code=8 THEN 'interrupted'
        ELSE 'failed' END = :outcome)
    AND date(started_at_ms / 1000,'unixepoch','localtime') BETWEEN :first_day AND :last_day";

fn valid_day(conn: &Connection, day: &str) -> Result<(), StoreError> {
    if day.len() != 10
        || !day.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return Err(super::fail(ErrorKind::Invalid));
    }
    let normalized: Option<String> =
        conn.query_row("SELECT date(?1,'+0 days')", [day], |r| r.get(0))?;
    if normalized.as_deref() != Some(day) {
        return Err(super::fail(ErrorKind::Invalid));
    }
    Ok(())
}

/// An indexable superset of the selected local dates. A nonexistent midnight
/// may normalize backwards, and some historical rollbacks even cross the date
/// boundary twice. The shared local-date predicate is authoritative, rather
/// than assuming every local date is one contiguous UTC interval. Neighboring
/// calendar dates give a conservative scan without fixed-length-day arithmetic.
fn bounds(conn: &Connection, start: &str, end: &str) -> Result<(i64, i64), StoreError> {
    let (start, end): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT unixepoch(?1,'-2 days','utc')*1000,unixepoch(?2,'+3 days','utc')*1000",
        params![start, end],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    start
        .zip(end)
        .ok_or_else(|| super::fail(ErrorKind::Invalid))
}

/// POSIX timezone state is refreshed when opening a workspace. No TZ env or
/// process-global environment mutation is needed by the app.
pub fn refresh_timezone() {
    // SAFETY: this declaration matches POSIX libc's argument-free `tzset`.
    unsafe extern "C" {
        fn tzset();
    }
    // SAFETY: tzset takes no arguments and initializes libc's timezone tables.
    // SQLite's localtime conversion reads this libc state; Rust has no safe API
    // for refreshing it after the platform timezone configuration changes.
    unsafe {
        tzset();
    }
}

#[tracing::instrument(target = "zeal::operations", name = "analytics.query", skip_all)]
pub fn query(data: &Path, ws: &Path, mut filter: Filter) -> Result<Analytics, StoreError> {
    use rusqlite::OptionalExtension;

    let ws = ws.canonicalize()?;
    let mut store = Store::open(data)?;
    let tx = store.conn.transaction()?;
    let today: String = tx.query_row("SELECT date('now','localtime')", [], |r| r.get(0))?;
    let start = filter
        .start
        .get_or_insert(tx.query_row("SELECT date(?1,'-29 days')", [&today], |r| r.get(0))?)
        .clone();
    let end = filter.end.get_or_insert(today.clone()).clone();
    valid_day(&tx, &start)?;
    valid_day(&tx, &end)?;
    let span: f64 = tx.query_row(
        "SELECT julianday(?2)-julianday(?1)+1",
        params![start, end],
        |r| r.get(0),
    )?;
    // Reject oversized requests explicitly; never silently truncate their data.
    if !(1.0..=366.0).contains(&span)
        || filter
            .cmd
            .as_deref()
            .is_some_and(|s| !["build", "test"].contains(&s))
        || filter
            .outcome
            .as_deref()
            .is_some_and(|s| !["ok", "failed", "interrupted"].contains(&s))
        // Matches the observation.target limit in store::events; the write
        // side (runner::validate_target) imposes no cap of its own.
        || filter.target.as_deref().is_some_and(|s| s.len() > 64 * 1024)
    {
        return Err(super::fail(ErrorKind::Invalid));
    }
    let (from, until) = bounds(&tx, &start, &end)?;
    let ws_id: Option<String> = tx
        .query_row(
            "SELECT id FROM workspace WHERE canonical_path=?1",
            [ws.as_os_str().as_encoded_bytes()],
            |r| r.get(0),
        )
        .optional()?;
    let mut days: Vec<Day> = tx
        .prepare(
            "WITH RECURSIVE calendar(day) AS (
            SELECT ?1 UNION ALL SELECT date(day,'+1 day') FROM calendar WHERE day < ?2
         ) SELECT day,0 FROM calendar ORDER BY day",
        )?
        .query_map(params![start, end], |r| {
            Ok(Day {
                day: r.get(0)?,
                count: 0,
                timed: 0,
                ok: 0,
                failed: 0,
                interrupted: 0,
                total_ms: 0,
                p50_ms: 0,
                p95_ms: 0,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut durations: Vec<Vec<i64>> = vec![Vec::new(); days.len()];
    let mut by_target: HashMap<String, TargetAgg> = HashMap::new();
    // Overflowing subtraction promotes to REAL rather than wrapping;
    // reading it back as i64 then fails closed as Corrupt.
    let sql = format!(
        "WITH records(id,cmd,target_raw,started_at_ms,finished_at_ms,exit_code) AS (
            SELECT id,cmd,target_raw,started_at_ms,finished_at_ms,exit_code
            FROM run WHERE workspace_id=:workspace
            UNION ALL
            SELECT o.id,json_extract(o.record,'$.cmd'),json_extract(o.record,'$.target'),
                json_extract(o.record,'$.startedAt'),json_extract(o.record,'$.finishedAt'),
                json_extract(o.record,'$.exitCode')
            FROM observation o WHERE o.workspace=:path AND o.complete=1
                AND NOT EXISTS (SELECT 1 FROM run r WHERE r.workspace_id=:workspace AND r.id=o.id)
        ) SELECT date(started_at_ms / 1000,'unixepoch','localtime'),exit_code,target_raw,
            finished_at_ms - started_at_ms FROM records WHERE {MATCH}"
    );
    let mut statement = tx.prepare(&sql)?;
    let rows = statement
        .query_map(
            named_params! {
                ":workspace":ws_id, ":path":ws.to_string_lossy(), ":start":from, ":end":until,
                ":first_day":start, ":last_day":end,
                ":cmd":filter.cmd, ":outcome":filter.outcome,
            },
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    for (day, exit_code, target, ms) in rows {
        // A clock jump backwards must not corrupt the series.
        let duration = ms.map(|ms| ms.max(0));
        let entry = by_target.entry(target.clone()).or_default();
        entry.runs += 1;
        if let Some(duration) = duration {
            entry.durations.push(duration);
        }
        if filter.target.as_deref().is_some_and(|t| t != target) {
            continue;
        }
        let i = days
            .binary_search_by(|d| d.day.cmp(&day))
            .map_err(|_| super::fail(ErrorKind::Invalid))?;
        let slot = &mut days[i];
        slot.count += 1;
        match exit_code {
            0 => slot.ok += 1,
            8 => slot.interrupted += 1,
            _ => slot.failed += 1,
        }
        if let Some(duration) = duration {
            slot.timed += 1;
            slot.total_ms = slot.total_ms.saturating_add(duration);
            durations[i].push(duration);
        }
    }
    for (i, sorted) in durations.iter_mut().enumerate() {
        sorted.sort_unstable();
        days[i].p50_ms = percentile(sorted, 50);
        days[i].p95_ms = percentile(sorted, 95);
    }
    let mut targets: Vec<TargetDuration> = by_target
        .into_iter()
        .map(|(target, agg)| {
            let mut sorted = agg.durations;
            sorted.sort_unstable();
            TargetDuration {
                target,
                runs: agg.runs,
                total_ms: sorted.iter().fold(0i64, |sum, ms| sum.saturating_add(*ms)),
                p50_ms: percentile(&sorted, 50),
                p95_ms: percentile(&sorted, 95),
            }
        })
        .collect();
    targets.sort_by(|a, b| b.total_ms.cmp(&a.total_ms).then(a.target.cmp(&b.target)));
    targets.truncate(TARGET_LIMIT);
    Ok(Analytics {
        filter,
        today,
        days,
        targets,
    })
}

const TARGET_LIMIT: usize = 10;

#[derive(Default)]
struct TargetAgg {
    runs: i64,
    durations: Vec<i64>,
}

/// Nearest-rank percentile over a sorted series; empty input reports zero.
fn percentile(sorted: &[i64], p: u64) -> i64 {
    debug_assert!((1..=100).contains(&p));
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() as u64 * p).div_ceil(100) - 1) as usize]
}

#[cfg(test)]
mod tests {
    use super::super::RunRecord;
    use super::*;
    use crate::protocol::{bep, bes};
    use prost::Message;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Instant;
    fn total(result: &Analytics) -> i64 {
        result.days.iter().map(|day| day.count).sum()
    }

    struct Fixture {
        root: PathBuf,
        data: PathBuf,
        ws: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let fixture = Self::empty();
            // Register workspace through M4; this unfinished record is excluded.
            super::super::create(&fixture.data, &fixture.ws, "build", "//:unfinished").unwrap();
            fixture
        }
        fn empty() -> Self {
            let root =
                std::env::temp_dir().join(format!("zeal-analytics-{}", uuid::Uuid::new_v4()));
            let data = root.join("data");
            let ws = root.join("workspace");
            fs::create_dir_all(&ws).unwrap();
            Self { root, data, ws }
        }
        fn millis(&self, time: &str) -> u64 {
            Store::open(&self.data)
                .unwrap()
                .conn
                .query_row("SELECT unixepoch(?1,'utc')*1000", [time], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap()
                .try_into()
                .unwrap()
        }
        fn observation(
            &self,
            started: &str,
            cmd: &str,
            target: &str,
            finished: Option<(&str, i32)>,
        ) -> RunRecord {
            RunRecord {
                id: uuid::Uuid::new_v4().to_string(),
                workspace: self
                    .ws
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                cmd: cmd.into(),
                target: target.into(),
                started_at: self.millis(started),
                finished_at: finished.map(|(time, _)| self.millis(time)),
                exit_code: finished.map(|(_, code)| code),
                external: true,
            }
        }
        #[allow(deprecated)]
        fn save_observation(&self, record: &RunRecord, complete: bool) -> bes::OrderedBuildEvent {
            let ordered = |sequence_number, event: bep::BuildEvent| bes::OrderedBuildEvent {
                stream_id: Some(bes::StreamId {
                    build_id: record.id.clone(),
                    invocation_id: record.id.clone(),
                    component: 3,
                }),
                sequence_number,
                event: Some(bes::BuildEvent {
                    bazel_event: Some(prost_types::Any {
                        type_url: "type.googleapis.com/build_event_stream.BuildEvent".into(),
                        value: event.encode_to_vec(),
                    }),
                    ..Default::default()
                }),
            };
            let mut last = ordered(
                1,
                bep::BuildEvent {
                    payload: Some(bep::build_event::Payload::Started(bep::BuildStarted {
                        uuid: record.id.clone(),
                        command: record.cmd.clone(),
                        workspace_directory: record.workspace.clone(),
                        start_time_millis: record.started_at as i64,
                        ..Default::default()
                    })),
                    children: vec![bep::BuildEventId {
                        id: Some(bep::build_event_id::Id::Pattern(
                            bep::build_event_id::PatternExpandedId {
                                pattern: vec![record.target.clone()],
                            },
                        )),
                    }],
                    last_message: complete && record.finished_at.is_none(),
                    ..Default::default()
                },
            );
            super::super::events::save(&self.data, &last).unwrap();
            if let Some(finished) = record.finished_at {
                last = ordered(
                    2,
                    bep::BuildEvent {
                        payload: Some(bep::build_event::Payload::Finished(bep::BuildFinished {
                            finish_time_millis: finished as i64,
                            exit_code: record.exit_code.map(|code| bep::build_finished::ExitCode {
                                code,
                                ..Default::default()
                            }),
                            ..Default::default()
                        })),
                        last_message: complete,
                        ..Default::default()
                    },
                );
                super::super::events::save(&self.data, &last).unwrap();
            }
            last
        }
        fn insert(&self, day: &str, cmd: &str, code: Option<i32>) -> String {
            self.insert_at(day, cmd, code, "//synthetic:target", None)
        }
        fn insert_at(
            &self,
            started: &str,
            cmd: &str,
            code: Option<i32>,
            target: &str,
            finished: Option<&str>,
        ) -> String {
            let store = Store::open(&self.data).unwrap();
            let ws_id = store
                .workspace_id(&self.ws.canonicalize().unwrap())
                .unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            store
                .conn
                .execute(
                    "INSERT INTO run(id,workspace_id,cmd,target_raw,started_at_ms,finished_at_ms,exit_code)
                VALUES(?1,?2,?3,?4,unixepoch(?5,'utc')*1000,unixepoch(?6,'utc')*1000,?7)",
                    params![id, ws_id, cmd, target, started, finished, code],
                )
                .unwrap();
            id
        }
        fn filter(&self) -> Filter {
            Filter {
                start: Some("2026-03-07".into()),
                end: Some("2026-03-09".into()),
                ..Filter::default()
            }
        }
        fn query(&self, filter: Filter) -> Analytics {
            query(&self.data, &self.ws, filter).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn terminal_only_workspace_alias_and_retries_count_once() {
        let f = Fixture::empty();
        let record = f.observation(
            "2026-03-07 12:00:00",
            "test",
            "//terminal:test",
            Some(("2026-03-07 12:00:12", 0)),
        );
        let last = f.save_observation(&record, true);
        super::super::events::save(&f.data, &last).unwrap();
        let workspace_count: i64 = Store::open(&f.data)
            .unwrap()
            .conn
            .query_row("SELECT count(*) FROM workspace", [], |r| r.get(0))
            .unwrap();
        assert_eq!(workspace_count, 0);
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        let mut unrelated = record.clone();
        unrelated.id = uuid::Uuid::new_v4().to_string();
        unrelated.workspace = other.to_string_lossy().into_owned();
        f.save_observation(&unrelated, true);
        let alias = f.root.join("alias");
        std::os::unix::fs::symlink(&f.ws, &alias).unwrap();
        let result = query(&f.data, &alias, f.filter()).unwrap();
        assert_eq!(total(&result), 1);
        assert_eq!((result.days[0].ok, result.days[0].timed), (1, 1));
        assert_eq!(result.days[0].total_ms, 12_000);
        assert_eq!(
            (result.days[0].p50_ms, result.days[0].p95_ms),
            (12_000, 12_000)
        );
        assert_eq!(result.targets[0].target, "//terminal:test");
    }

    #[test]
    fn terminal_stream_must_be_complete_with_a_build_or_test_exit() {
        let f = Fixture::empty();
        let incomplete = f.observation(
            "2026-03-07 12:00:00",
            "build",
            "//:incomplete",
            Some(("2026-03-07 12:00:01", 0)),
        );
        f.save_observation(&incomplete, false);
        let mut missing_exit = incomplete.clone();
        missing_exit.id = uuid::Uuid::new_v4().to_string();
        missing_exit.exit_code = None;
        f.save_observation(&missing_exit, true);
        let unfinished = f.observation("2026-03-07 12:00:00", "test", "//:unfinished", None);
        f.save_observation(&unfinished, false);
        let run = f.observation(
            "2026-03-07 12:00:00",
            "run",
            "//:program",
            Some(("2026-03-07 12:00:01", 0)),
        );
        f.save_observation(&run, true);
        let outside = f.observation(
            "2026-03-10 00:00:00",
            "test",
            "//:outside",
            Some(("2026-03-10 00:00:01", 0)),
        );
        f.save_observation(&outside, true);
        let result = f.query(f.filter());
        assert_eq!(total(&result), 0);
        assert!(result.targets.is_empty());
    }

    #[test]
    fn gui_precedence_is_applied_before_date_command_outcome_and_target_filters() {
        let f = Fixture::new();
        let gui = f.insert_at(
            "2026-03-07 12:00:00",
            "build",
            Some(1),
            "//:gui",
            Some("2026-03-07 12:00:10"),
        );
        let unfinished = f.insert("2026-03-07 12:00:00", "build", None);
        let run = f.insert("2026-03-07 12:00:00", "run", Some(0));
        let outside = f.insert("2026-03-06 12:00:00", "build", Some(0));
        for id in [gui, unfinished, run, outside] {
            let mut observation = f.observation(
                "2026-03-07 12:00:00",
                "test",
                "//:terminal",
                Some(("2026-03-07 12:00:30", 0)),
            );
            observation.id = id;
            f.save_observation(&observation, true);
        }
        let result = f.query(f.filter());
        assert_eq!(total(&result), 1);
        assert_eq!(
            (result.days[0].failed, result.days[0].total_ms),
            (1, 10_000)
        );
        assert_eq!(result.targets[0].target, "//:gui");
        let terminal = f.query(Filter {
            cmd: Some("test".into()),
            outcome: Some("ok".into()),
            ..f.filter()
        });
        assert_eq!(total(&terminal), 0);
        assert!(terminal.targets.is_empty());
        let target = f.query(Filter {
            target: Some("//:terminal".into()),
            ..f.filter()
        });
        assert_eq!(total(&target), 0);
        assert_eq!(target.targets[0].target, "//:gui");
    }

    #[test]
    fn terminal_and_gui_runs_share_duration_filters_and_rankings() {
        let f = Fixture::new();
        f.insert_at(
            "2026-03-07 10:00:00",
            "build",
            Some(0),
            "//a:x",
            Some("2026-03-07 10:00:10"),
        );
        f.save_observation(
            &f.observation(
                "2026-03-07 11:00:00",
                "test",
                "//a:x",
                Some(("2026-03-07 11:00:30", 0)),
            ),
            true,
        );
        f.save_observation(
            &f.observation(
                "2026-03-07 12:00:00",
                "build",
                "//b:y",
                Some(("2026-03-07 12:00:20", 1)),
            ),
            true,
        );
        f.save_observation(
            &f.observation(
                "2026-03-08 12:00:00",
                "test",
                "//a:x",
                Some(("2026-03-08 11:59:59", 8)),
            ),
            true,
        );
        let result = f.query(f.filter());
        assert_eq!(total(&result), 4);
        assert_eq!(result.days[0].total_ms, 60_000);
        assert_eq!(
            (result.days[0].p50_ms, result.days[0].p95_ms),
            (20_000, 30_000)
        );
        assert_eq!((result.days[1].interrupted, result.days[1].timed), (1, 1));
        assert_eq!(result.days[1].total_ms, 0);
        assert_eq!(result.targets[0].target, "//a:x");
        assert_eq!(
            (result.targets[0].runs, result.targets[0].total_ms),
            (3, 40_000)
        );
        assert_eq!(
            (result.targets[0].p50_ms, result.targets[0].p95_ms),
            (10_000, 30_000)
        );
        let scoped = f.query(Filter {
            target: Some("//a:x".into()),
            ..f.filter()
        });
        assert_eq!(scoped.days[0].count, 2);
        assert_eq!(scoped.days[0].total_ms, 40_000);
        assert_eq!(scoped.targets.len(), 2);
        let failed = f.query(Filter {
            cmd: Some("build".into()),
            outcome: Some("failed".into()),
            ..f.filter()
        });
        assert_eq!(total(&failed), 1);
        assert_eq!(failed.targets[0].target, "//b:y");
        assert_eq!(failed.days[0].total_ms, 20_000);
    }

    #[test]
    fn counts_follow_workspace_range_command_and_outcome() {
        let f = Fixture::new();
        f.insert("2026-03-07 00:00:00", "build", Some(0));
        f.insert("2026-03-07 23:59:59", "test", Some(8));
        f.insert("2026-03-09 23:59:59", "build", Some(1));
        f.insert("2026-03-06 23:59:59", "build", Some(0));
        f.insert("2026-03-10 00:00:00", "build", Some(0));
        f.insert("2026-03-07 12:00:00", "build", None);
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        let (rec, _) = super::super::create(&f.data, &other, "build", "//:other").unwrap();
        super::super::finish(&f.data, &other, &rec.id, 0).unwrap();
        let store = Store::open(&f.data).unwrap();
        store
            .conn
            .execute(
                "UPDATE run SET started_at_ms=unixepoch('2026-03-07','utc')*1000 WHERE id=?1",
                [rec.id],
            )
            .unwrap();
        drop(store);
        let all = f.query(f.filter());
        assert_eq!(
            all.days.iter().map(|d| d.count).collect::<Vec<_>>(),
            vec![2, 0, 1]
        );
        assert_eq!(total(&all), 3);
        let selected = f.query(Filter {
            cmd: Some("build".into()),
            outcome: Some("ok".into()),
            ..f.filter()
        });
        assert_eq!(total(&selected), 1);
        let interrupted = f.query(Filter {
            outcome: Some("interrupted".into()),
            ..f.filter()
        });
        assert_eq!(total(&interrupted), 1);
        let failed = f.query(Filter {
            outcome: Some("failed".into()),
            ..f.filter()
        });
        assert_eq!(total(&failed), 1);
    }
    #[test]
    fn durations_outcomes_and_target_ranking() {
        let f = Fixture::new();
        f.insert_at(
            "2026-03-07 10:00:00",
            "build",
            Some(0),
            "//a:x",
            Some("2026-03-07 10:00:10"),
        );
        f.insert_at(
            "2026-03-07 11:00:00",
            "build",
            Some(0),
            "//a:x",
            Some("2026-03-07 11:00:30"),
        );
        f.insert_at(
            "2026-03-07 12:00:00",
            "test",
            Some(1),
            "//b:y",
            Some("2026-03-07 12:00:20"),
        );
        f.insert("2026-03-07 13:00:00", "build", None);
        f.insert_at("2026-03-08 12:00:00", "build", Some(8), "//a:x", None);
        let result = f.query(f.filter());
        let first = &result.days[0];
        assert_eq!(first.count, 3);
        assert_eq!((first.ok, first.failed, first.interrupted), (2, 1, 0));
        assert_eq!(first.timed, 3);
        assert_eq!(first.total_ms, 60_000);
        assert_eq!((first.p50_ms, first.p95_ms), (20_000, 30_000));
        let second = &result.days[1];
        assert_eq!((second.count, second.interrupted), (1, 1));
        assert_eq!((second.timed, second.total_ms), (0, 0));
        assert_eq!((second.p50_ms, second.p95_ms), (0, 0));
        assert_eq!(result.targets.len(), 2);
        assert_eq!(result.targets[0].target, "//a:x");
        assert_eq!(result.targets[0].runs, 3);
        assert_eq!(result.targets[0].total_ms, 40_000);
        assert_eq!(result.targets[0].p50_ms, 10_000);
        assert_eq!(result.targets[1].target, "//b:y");
        assert_eq!(result.targets[1].runs, 1);
        // The target filter narrows the daily series but not the ranking.
        let scoped = f.query(Filter {
            target: Some("//a:x".into()),
            ..f.filter()
        });
        assert_eq!(scoped.days[0].count, 2);
        assert_eq!(scoped.days[0].total_ms, 40_000);
        assert_eq!(scoped.days[1].count, 1);
        assert_eq!(scoped.targets.len(), 2);
        // The ranking follows the cmd/outcome filters too.
        let failed = f.query(Filter {
            outcome: Some("failed".into()),
            ..f.filter()
        });
        assert_eq!(failed.targets.len(), 1);
        assert_eq!(failed.targets[0].target, "//b:y");
        assert_eq!(
            query(
                &f.data,
                &f.ws,
                Filter {
                    target: Some("x".repeat(64 * 1024 + 1)),
                    ..f.filter()
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::Invalid
        );
    }
    #[test]
    fn target_ranking_is_ordered_and_truncated() {
        let f = Fixture::new();
        for i in 0..12 {
            f.insert_at(
                "2026-03-07 12:00:00",
                "build",
                Some(0),
                &format!("//t:{i:02}"),
                Some("2026-03-07 12:00:01"),
            );
        }
        // One heavier run and a same-total pair to pin the label tie-break.
        f.insert_at(
            "2026-03-07 12:00:00",
            "build",
            Some(0),
            "//t:heavy",
            Some("2026-03-07 12:00:05"),
        );
        f.insert_at(
            "2026-03-08 12:00:00",
            "build",
            Some(0),
            "//t:extra",
            Some("2026-03-08 12:00:01"),
        );
        let result = f.query(f.filter());
        assert_eq!(result.targets.len(), TARGET_LIMIT);
        assert_eq!(result.targets[0].target, "//t:heavy");
        // Every 1s target ties; the byte order of the label breaks the tie.
        let tied: Vec<&str> = result.targets[1..]
            .iter()
            .map(|t| t.target.as_str())
            .collect();
        assert_eq!(
            tied,
            [
                "//t:00", "//t:01", "//t:02", "//t:03", "//t:04", "//t:05", "//t:06", "//t:07",
                "//t:08"
            ]
        );
    }
    #[test]
    fn artifacts_and_missing_finish_do_not_affect_counts_and_late_records_are_visible() {
        let f = Fixture::new();
        let (rec, dir) = super::super::create(&f.data, &f.ws, "build", "//:app").unwrap();
        super::super::finish(&f.data, &f.ws, &rec.id, 0).unwrap();
        let store = Store::open(&f.data).unwrap();
        store.conn.execute("UPDATE run SET started_at_ms=unixepoch('2026-03-07','utc')*1000,finished_at_ms=NULL WHERE id=?1",[rec.id]).unwrap();
        drop(store);
        assert_eq!(total(&f.query(f.filter())), 1);
        fs::remove_dir_all(dir).unwrap();
        assert_eq!(total(&f.query(f.filter())), 1);
        f.insert("2026-03-07 12:00:00", "test", Some(0));
        assert_eq!(total(&f.query(f.filter())), 2);
    }
    #[test]
    fn defaults_empty_workspace_alias_and_invalid_bound_values() {
        let f = Fixture::new();
        let result = f.query(Filter::default());
        assert_eq!(result.days.len(), 30);
        assert_eq!(total(&result), 0);
        assert_eq!(result.filter.end, Some(result.today));
        let other = f.root.join("unregistered");
        fs::create_dir(&other).unwrap();
        assert_eq!(total(&query(&f.data, &other, f.filter()).unwrap()), 0);
        let alias = f.root.join("alias");
        std::os::unix::fs::symlink(&f.ws, &alias).unwrap();
        f.insert("2026-03-07", "build", Some(0));
        assert_eq!(total(&query(&f.data, &alias, f.filter()).unwrap()), 1);
        for filter in [
            Filter {
                start: Some("2026-02-30".into()),
                ..f.filter()
            },
            Filter {
                end: Some("2026-03-06".into()),
                ..f.filter()
            },
            Filter {
                start: Some("2024-01-01".into()),
                ..f.filter()
            },
            Filter {
                cmd: Some("build' OR 1=1 --".into()),
                ..f.filter()
            },
            Filter {
                outcome: Some("ok' OR 1=1 --".into()),
                ..f.filter()
            },
        ] {
            assert_eq!(
                query(&f.data, &f.ws, filter).unwrap_err().kind,
                ErrorKind::Invalid
            );
        }
        assert_eq!(total(&f.query(f.filter())), 1);
    }
    // Each timezone is a separate process. Changing TZ inside a multithreaded
    // Rust test process would be unsafe and contaminate unrelated tests.
    #[test]
    fn timezone_boundaries_in_isolated_processes() {
        for zone in [
            "America/New_York",
            "Asia/Tokyo",
            "UTC",
            "Asia/Beirut",
            "America/Havana",
            "Pacific/Apia",
            "America/Goose_Bay",
        ] {
            assert!(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "store::analytics::tests::timezone_probe",
                        "--exact",
                        "--ignored"
                    ])
                    .env("TZ", zone)
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
    #[test]
    #[ignore = "invoked with an explicit TZ by timezone_boundaries_in_isolated_processes"]
    fn timezone_probe() {
        refresh_timezone();
        let f = Fixture::new();

        let zone = std::env::var("TZ").unwrap();
        let spring: (i64, i64) = Store::open(&f.data)
            .unwrap()
            .conn
            .query_row(
                "SELECT unixepoch('2026-03-08','utc')*1000,unixepoch('2026-03-09','utc')*1000",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let spring = if zone == "America/New_York" || zone == "America/Havana" {
            Store::open(&f.data).unwrap().conn.query_row("SELECT unixepoch('2026-03-08 05:00:00')*1000,unixepoch('2026-03-09 04:00:00')*1000",[],|r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).unwrap()
        } else {
            spring
        };
        if ["America/New_York", "America/Havana", "America/Goose_Bay"].contains(&zone.as_str()) {
            assert_eq!(spring.1 - spring.0, 23 * 3_600_000);
        } else {
            assert_eq!(spring.1 - spring.0, 24 * 3_600_000);
        }
        if zone == "Asia/Beirut" {
            f.insert("2026-03-28 23:30:00", "build", Some(0));
            f.insert("2026-03-29 01:00:00", "build", Some(0));
            let result = f.query(Filter {
                start: Some("2026-03-29".into()),
                end: Some("2026-03-29".into()),
                ..Filter::default()
            });
            assert_eq!(total(&result), 1);
        }
        if zone == "Pacific/Apia" {
            // The calendar skipped December 30, 2011 entirely.
            let skipped = f.query(Filter {
                start: Some("2011-12-30".into()),
                end: Some("2011-12-30".into()),
                ..Filter::default()
            });
            assert_eq!(total(&skipped), 0);
        }
        let expected = f.insert("2026-03-08 12:00:00", "build", Some(0));
        let last = f.insert("2026-03-08 23:59:59", "build", Some(0));
        let sql = "UPDATE run SET started_at_ms=?1 WHERE id=?2";
        Store::open(&f.data)
            .unwrap()
            .conn
            .execute(sql, params![spring.0, expected])
            .unwrap();
        // One millisecond before each exclusive boundary must stay on that day.
        Store::open(&f.data)
            .unwrap()
            .conn
            .execute(sql, params![spring.1 - 1, last])
            .unwrap();
        let before = f.insert("2026-03-07", "build", Some(0));
        Store::open(&f.data)
            .unwrap()
            .conn
            .execute(sql, params![spring.0 - 1, before])
            .unwrap();
        f.insert("2026-03-09 00:00:00", "build", Some(0));
        let result = f.query(Filter {
            start: Some("2026-03-08".into()),
            end: Some("2026-03-08".into()),
            ..Filter::default()
        });
        assert_eq!(total(&result), 2);
        let result = f.query(Filter {
            start: Some("2026-11-01".into()),
            end: Some("2026-11-01".into()),
            ..Filter::default()
        });
        assert_eq!(total(&result), 0);
        if zone == "America/New_York" || zone == "America/Havana" {
            let (from, until): (i64, i64) = Store::open(&f.data).unwrap().conn.query_row(
                    "SELECT unixepoch('2026-11-01 04:00:00')*1000,unixepoch('2026-11-02 05:00:00')*1000",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(until - from, 25 * 3_600_000);
            let first = f.insert("2026-11-01 12:00:00", "build", Some(0));
            let last = f.insert("2026-11-01 12:01:00", "build", Some(0));
            Store::open(&f.data)
                .unwrap()
                .conn
                .execute(sql, params![from, first])
                .unwrap();
            Store::open(&f.data)
                .unwrap()
                .conn
                .execute(sql, params![until - 1, last])
                .unwrap();
            let selected = f.query(Filter {
                start: Some("2026-11-01".into()),
                end: Some("2026-11-01".into()),
                ..Filter::default()
            });
            assert_eq!(total(&selected), 2);
        }
        if zone == "America/Goose_Bay" {
            // In 1988 the clock rolled from Oct 30 00:01 back into Oct 29.
            // Oct 30 is not a contiguous UTC interval. Both its short early
            // occurrence and its later occurrence count, but Oct 29 does not.
            let early = f.insert("1988-10-30 12:00:00", "build", Some(0));
            let previous = f.insert("1988-10-30 12:01:00", "build", Some(0));
            Store::open(&f.data).unwrap().conn.execute("UPDATE run SET started_at_ms=unixepoch('1988-10-30 02:00:30')*1000 WHERE id=?1",[early]).unwrap();
            Store::open(&f.data).unwrap().conn.execute("UPDATE run SET started_at_ms=unixepoch('1988-10-30 03:00:00')*1000 WHERE id=?1",[previous]).unwrap();
            f.insert("1988-10-30 12:02:00", "build", Some(0));
            let selected = f.query(Filter {
                start: Some("1988-10-30".into()),
                end: Some("1988-10-30".into()),
                ..Filter::default()
            });
            assert_eq!(total(&selected), 2);
        }
    }
    #[test]
    #[ignore = "manual synthetic 5,000-record measurement; run with --ignored --nocapture"]
    fn measure_5000_records() {
        let f = Fixture::new();
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        super::super::create(&f.data, &other, "test", "//:unfinished").unwrap();
        let mut store = Store::open(&f.data).unwrap();
        let ids = [
            store.workspace_id(&f.ws.canonicalize().unwrap()).unwrap(),
            store.workspace_id(&other.canonicalize().unwrap()).unwrap(),
        ];
        let tx = store.conn.transaction().unwrap();
        for i in 0..5000 {
            tx.execute("INSERT INTO run(id,workspace_id,cmd,target_raw,started_at_ms,exit_code) VALUES(?1,?2,?3,'//synthetic:target',unixepoch('2026-03-01',?4,'utc')*1000,?5)",
                params![uuid::Uuid::new_v4().to_string(),ids[i%2],if i%4<2 {"build"} else {"test"},format!("+{} days",i%30),[0,1,8][i%3]]).unwrap();
        }
        tx.commit().unwrap();
        drop(store);
        let filter = Filter {
            start: Some("2026-03-01".into()),
            end: Some("2026-03-30".into()),
            ..Filter::default()
        };
        for i in 0..10 {
            let at = Instant::now();
            let result = f.query(filter.clone());
            assert_eq!(total(&result), 2500);
            let bytes = serde_json::to_vec(&result).unwrap().len();
            eprintln!("initial[{i}]={:?}, bytes={bytes}", at.elapsed());
            let at = Instant::now();
            let result = f.query(Filter {
                cmd: Some("test".into()),
                outcome: Some("failed".into()),
                ..filter.clone()
            });
            assert_eq!(total(&result), 416);
            eprintln!(
                "refilter[{i}]={:?}, days={}",
                at.elapsed(),
                result.days.len()
            );
        }
    }
}
