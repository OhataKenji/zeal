//! Read-only projections of Bazel invocation facts.
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

// Local-date and outcome rules apply to the common invocation table.
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

    let ws = super::database::workspace_path(ws)?;
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
        // Matches the target fact limit in store::events.
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
        "SELECT date(started_at_ms / 1000,'unixepoch','localtime'),exit_code,target_raw,
            finished_at_ms - started_at_ms FROM run WHERE workspace_id=:workspace AND {MATCH}"
    );
    let mut statement = tx.prepare(&sql)?;
    let rows = statement
        .query_map(
            named_params! {
                ":workspace":ws_id, ":start":from, ":end":until,
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
    use super::*;
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
            fixture.register(&fixture.ws);
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
        fn register(&self, ws: &Path) -> String {
            let store = Store::open(&self.data).unwrap();
            let path = ws.canonicalize().unwrap();
            let proposed = uuid::Uuid::new_v4().to_string();
            store.conn.execute("INSERT INTO workspace(id,canonical_path) VALUES(?1,?2) ON CONFLICT(canonical_path) DO NOTHING",params![proposed,path.as_os_str().as_encoded_bytes()]).unwrap();
            store
                .conn
                .query_row(
                    "SELECT id FROM workspace WHERE canonical_path=?1",
                    [path.as_os_str().as_encoded_bytes()],
                    |r| r.get(0),
                )
                .unwrap()
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
            let ws_id: String = store
                .conn
                .query_row(
                    "SELECT id FROM workspace WHERE canonical_path=?1",
                    [self
                        .ws
                        .canonicalize()
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes()],
                    |r| r.get(0),
                )
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
        fn set_started_at(&self, id: &str, millis: i64) {
            Store::open(&self.data)
                .unwrap()
                .conn
                .execute(
                    "UPDATE run SET started_at_ms=?1 WHERE id=?2",
                    params![millis, id],
                )
                .unwrap();
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
        let other_id = f.register(&other);
        Store::open(&f.data).unwrap().conn.execute("INSERT INTO run(id,workspace_id,cmd,target_raw,started_at_ms,exit_code) VALUES(?1,?2,'build','//:other',unixepoch('2026-03-07','utc')*1000,0)",params![uuid::Uuid::new_v4().to_string(),other_id]).unwrap();
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
    fn daily_duration_statistics_use_only_observed_runs() {
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

        let result = f.query(f.filter());
        let first = &result.days[0];
        assert_eq!(first.count, 3);
        assert_eq!((first.ok, first.failed, first.interrupted), (2, 1, 0));
        assert_eq!(first.timed, 3);
        assert_eq!(first.total_ms, 60_000);
        assert_eq!((first.p50_ms, first.p95_ms), (20_000, 30_000));
    }

    #[test]
    fn untimed_runs_keep_outcome_counts_without_duration_statistics() {
        let f = Fixture::new();
        f.insert_at("2026-03-08 12:00:00", "build", Some(8), "//a:x", None);

        let result = f.query(f.filter());
        let second = &result.days[1];
        assert_eq!((second.count, second.interrupted), (1, 1));
        assert_eq!((second.timed, second.total_ms), (0, 0));
        assert_eq!((second.p50_ms, second.p95_ms), (0, 0));
    }

    #[test]
    fn target_ranking_aggregates_runs_across_days() {
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
        f.insert_at("2026-03-08 12:00:00", "build", Some(8), "//a:x", None);
        f.insert_at(
            "2026-03-07 12:00:00",
            "test",
            Some(1),
            "//b:y",
            Some("2026-03-07 12:00:20"),
        );

        f.insert("2026-03-07 13:00:00", "build", None);

        let result = f.query(f.filter());
        assert_eq!(result.targets.len(), 2);
        assert_eq!(result.targets[0].target, "//a:x");
        assert_eq!(result.targets[0].runs, 3);
        assert_eq!(result.targets[0].total_ms, 40_000);
        assert_eq!(result.targets[0].p50_ms, 10_000);
        assert_eq!(result.targets[1].target, "//b:y");
        assert_eq!(result.targets[1].runs, 1);
    }

    #[test]
    fn target_filter_narrows_daily_series_but_not_ranking() {
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
        f.insert_at("2026-03-08 12:00:00", "build", Some(8), "//a:x", None);
        f.insert_at("2026-03-07 12:00:00", "test", Some(1), "//b:y", None);

        let scoped = f.query(Filter {
            target: Some("//a:x".into()),
            ..f.filter()
        });
        assert_eq!(scoped.days[0].count, 2);
        assert_eq!(scoped.days[0].total_ms, 40_000);
        assert_eq!(scoped.days[1].count, 1);
        assert_eq!(scoped.targets.len(), 2);
    }

    #[test]
    fn outcome_filter_narrows_target_ranking() {
        let f = Fixture::new();
        f.insert_at("2026-03-07 10:00:00", "build", Some(0), "//a:x", None);
        f.insert_at("2026-03-08 12:00:00", "build", Some(8), "//a:x", None);
        f.insert_at("2026-03-07 12:00:00", "test", Some(1), "//b:y", None);

        let failed = f.query(Filter {
            outcome: Some("failed".into()),
            ..f.filter()
        });
        assert_eq!(failed.targets.len(), 1);
        assert_eq!(failed.targets[0].target, "//b:y");
    }

    #[test]
    fn oversized_target_filter_is_rejected() {
        let f = Fixture::new();
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
    fn missing_finish_and_details_do_not_affect_observed_exit_counts() {
        let f = Fixture::new();
        f.insert("2026-03-07", "build", Some(0));
        assert_eq!(total(&f.query(f.filter())), 1);
        f.insert("2026-03-07", "test", Some(0));
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
    fn run_timezone_case(zone: &str, case: &str) {
        let test = format!("store::analytics::tests::{case}");
        let status = Command::new(std::env::current_exe().unwrap())
            .args([test.as_str(), "--exact", "--ignored"])
            .env("TZ", zone)
            .status()
            .unwrap();
        assert!(status.success(), "{case} failed in {zone}");
    }

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
            run_timezone_case(zone, "spring_day_boundaries");
        }
        run_timezone_case("America/New_York", "autumn_day_boundaries");
        run_timezone_case("America/Havana", "autumn_day_boundaries");
        run_timezone_case("Asia/Beirut", "midnight_gap_keeps_the_new_day");
        run_timezone_case("Pacific/Apia", "skipped_calendar_day_is_empty");
        run_timezone_case(
            "America/Goose_Bay",
            "repeated_day_keeps_disjoint_occurrences",
        );
    }

    #[test]
    #[ignore = "invoked with an explicit TZ by timezone_boundaries_in_isolated_processes"]
    fn spring_day_boundaries() {
        refresh_timezone();
        let f = Fixture::new();
        let zone = std::env::var("TZ").unwrap();
        let (sql, hours) = match zone.as_str() {
            "America/New_York" | "America/Havana" => (
                "SELECT unixepoch('2026-03-08 05:00:00')*1000,unixepoch('2026-03-09 04:00:00')*1000",
                23,
            ),
            "America/Goose_Bay" => (
                "SELECT unixepoch('2026-03-08','utc')*1000,unixepoch('2026-03-09','utc')*1000",
                23,
            ),
            _ => (
                "SELECT unixepoch('2026-03-08','utc')*1000,unixepoch('2026-03-09','utc')*1000",
                24,
            ),
        };
        let (from, until): (i64, i64) = Store::open(&f.data)
            .unwrap()
            .conn
            .query_row(sql, [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(until - from, hours * 3_600_000);

        let first = f.insert("2026-03-08 12:00:00", "build", Some(0));
        let last = f.insert("2026-03-08 23:59:59", "build", Some(0));
        let before = f.insert("2026-03-07", "build", Some(0));
        f.set_started_at(&first, from);
        // One millisecond before each exclusive boundary must stay on that day.
        f.set_started_at(&last, until - 1);
        f.set_started_at(&before, from - 1);
        f.insert("2026-03-09 00:00:00", "build", Some(0));

        let result = f.query(Filter {
            start: Some("2026-03-08".into()),
            end: Some("2026-03-08".into()),
            ..Filter::default()
        });
        assert_eq!(total(&result), 2);
        let empty = f.query(Filter {
            start: Some("2026-11-01".into()),
            end: Some("2026-11-01".into()),
            ..Filter::default()
        });
        assert_eq!(total(&empty), 0);
    }

    #[test]
    #[ignore = "invoked with an explicit TZ by timezone_boundaries_in_isolated_processes"]
    fn autumn_day_boundaries() {
        refresh_timezone();
        let f = Fixture::new();
        let (from, until): (i64, i64) = Store::open(&f.data).unwrap().conn.query_row(
            "SELECT unixepoch('2026-11-01 04:00:00')*1000,unixepoch('2026-11-02 05:00:00')*1000",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(until - from, 25 * 3_600_000);
        let first = f.insert("2026-11-01 12:00:00", "build", Some(0));
        let last = f.insert("2026-11-01 12:01:00", "build", Some(0));
        f.set_started_at(&first, from);
        f.set_started_at(&last, until - 1);

        let selected = f.query(Filter {
            start: Some("2026-11-01".into()),
            end: Some("2026-11-01".into()),
            ..Filter::default()
        });
        assert_eq!(total(&selected), 2);
    }

    #[test]
    #[ignore = "invoked with an explicit TZ by timezone_boundaries_in_isolated_processes"]
    fn midnight_gap_keeps_the_new_day() {
        refresh_timezone();
        let f = Fixture::new();
        f.insert("2026-03-28 23:30:00", "build", Some(0));
        f.insert("2026-03-29 01:00:00", "build", Some(0));
        let result = f.query(Filter {
            start: Some("2026-03-29".into()),
            end: Some("2026-03-29".into()),
            ..Filter::default()
        });
        assert_eq!(total(&result), 1);
    }

    #[test]
    #[ignore = "invoked with an explicit TZ by timezone_boundaries_in_isolated_processes"]
    fn skipped_calendar_day_is_empty() {
        refresh_timezone();
        let f = Fixture::new();
        // The calendar skipped December 30, 2011 entirely.
        let skipped = f.query(Filter {
            start: Some("2011-12-30".into()),
            end: Some("2011-12-30".into()),
            ..Filter::default()
        });
        assert_eq!(total(&skipped), 0);
    }

    #[test]
    #[ignore = "invoked with an explicit TZ by timezone_boundaries_in_isolated_processes"]
    fn repeated_day_keeps_disjoint_occurrences() {
        refresh_timezone();
        let f = Fixture::new();
        // In 1988 the clock rolled from Oct 30 00:01 back into Oct 29.
        // Oct 30 is not a contiguous UTC interval. Both its short early
        // occurrence and its later occurrence count, but Oct 29 does not.
        let early = f.insert("1988-10-30 12:00:00", "build", Some(0));
        let previous = f.insert("1988-10-30 12:01:00", "build", Some(0));
        Store::open(&f.data)
            .unwrap()
            .conn
            .execute(
                "UPDATE run SET started_at_ms=unixepoch('1988-10-30 02:00:30')*1000 WHERE id=?1",
                [early],
            )
            .unwrap();
        Store::open(&f.data)
            .unwrap()
            .conn
            .execute(
                "UPDATE run SET started_at_ms=unixepoch('1988-10-30 03:00:00')*1000 WHERE id=?1",
                [previous],
            )
            .unwrap();
        f.insert("1988-10-30 12:02:00", "build", Some(0));
        let selected = f.query(Filter {
            start: Some("1988-10-30".into()),
            end: Some("1988-10-30".into()),
            ..Filter::default()
        });
        assert_eq!(total(&selected), 2);
    }
    #[test]
    #[ignore = "manual synthetic 5,000-record measurement; run with --ignored --nocapture"]
    fn measure_5000_records() {
        let f = Fixture::new();
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        f.register(&other);
        let mut store = Store::open(&f.data).unwrap();
        let ids = [
            store
                .conn
                .query_row(
                    "SELECT id FROM workspace WHERE canonical_path=?1",
                    [f.ws.canonicalize().unwrap().as_os_str().as_encoded_bytes()],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
            store
                .conn
                .query_row(
                    "SELECT id FROM workspace WHERE canonical_path=?1",
                    [other.canonicalize().unwrap().as_os_str().as_encoded_bytes()],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
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
