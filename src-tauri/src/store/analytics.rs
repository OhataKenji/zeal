//! Read-only projections of Bazel invocation facts.
//! Artifacts and live runner state never participate in counting or filtering.
use super::{ErrorKind, Store, StoreError};
use rusqlite::{Connection, named_params, params};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Filter {
    pub start: Option<String>,
    pub end: Option<String>,
    pub cmd: Option<String>,
    pub outcome: Option<String>,
    /// Exact `target_pattern` match. Restricts the daily series only; the
    /// pattern ranking always reflects every pattern in the filtered period.
    pub pattern: Option<String>,
    pub workspace_id: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub day: String,
    pub count: i64,
    pub ok: i64,
    pub failed: i64,
    pub interrupted: i64,
    pub unknown: i64,
    /// Invocations carrying a recorded finish time; the denominator of the
    /// duration series. `count - timed` rows contribute nothing to it.
    pub timed: i64,
    /// Wall-clock total of invocations with a recorded finish time.
    pub total_ms: i64,
    /// Nearest-rank percentiles over the day's recorded durations.
    pub p50_ms: i64,
    pub p95_ms: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatternDuration {
    pub pattern: String,
    pub invocations: i64,
    pub total_ms: i64,
    pub p50_ms: i64,
    pub p95_ms: i64,
}
#[derive(Debug, Serialize)]
pub struct Workspace {
    pub id: String,
    pub path: String,
}
#[derive(Debug, Serialize)]
pub struct Analytics {
    pub filter: Filter,
    pub today: String,
    pub days: Vec<Day>,
    pub patterns: Vec<PatternDuration>,
    pub workspaces: Vec<Workspace>,
}

// Local-date and outcome rules apply to the common invocation table.
const MATCH: &str = "started_at_ms >= :start AND started_at_ms < :end
    AND (:cmd IS NULL OR cmd=:cmd)
    AND (:outcome IS NULL OR
        CASE WHEN exit_code IS NULL THEN 'unknown' WHEN exit_code=0 THEN 'ok' WHEN exit_code=8 THEN 'interrupted'
        ELSE 'failed' END = :outcome)
    AND date(started_at_ms / 1000,'unixepoch','localtime') BETWEEN :first_day AND :last_day";

const SCOPE: &str = "workspace_id IN (
    SELECT id FROM workspace
    WHERE (id=:workspace OR repo_url=(SELECT repo_url FROM workspace WHERE id=:workspace))
      AND (:selected_workspace IS NULL OR id=:selected_workspace)
)";

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
            .is_some_and(|s| s.is_empty() || s.len() > 64 || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
        || filter
            .outcome
            .as_deref()
            .is_some_and(|s| !["ok", "failed", "interrupted", "unknown"].contains(&s))
        // Matches the target-pattern fact limit in store::events.
        || filter.pattern.as_deref().is_some_and(|s| s.len() > 64 * 1024)
    {
        return Err(super::fail(ErrorKind::Invalid));
    }
    let ws_id: Option<String> = tx
        .query_row(
            "SELECT id FROM workspace WHERE canonical_path=?1",
            [ws.as_os_str().as_encoded_bytes()],
            |r| r.get(0),
        )
        .optional()?;
    let workspaces = workspace_candidates(&tx, ws_id.as_ref())?;
    if let Some(selected) = &filter.workspace_id
        && !workspaces.iter().any(|workspace| &workspace.id == selected)
    {
        return Err(super::fail(ErrorKind::Invalid));
    }
    let days: Vec<Day> = tx
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
                unknown: 0,
                total_ms: 0,
                p50_ms: 0,
                p95_ms: 0,
            })
        })?
        .collect::<Result<_, _>>()?;
    let (days, patterns) = aggregate(&tx, ws_id.as_ref(), &filter, &start, &end, days)?;
    Ok(Analytics {
        filter,
        today,
        days,
        patterns,
        workspaces,
    })
}

fn workspace_candidates(
    tx: &Connection,
    ws_id: Option<&String>,
) -> Result<Vec<Workspace>, StoreError> {
    let mut statement = tx.prepare(
        "SELECT id,canonical_path FROM workspace
         WHERE id=:workspace OR repo_url=(SELECT repo_url FROM workspace WHERE id=:workspace)
         ORDER BY canonical_path,id",
    )?;
    let rows = statement.query_map(named_params![":workspace": ws_id], |row| {
        let path: Vec<u8> = row.get(1)?;
        Ok(Workspace {
            id: row.get(0)?,
            path: PathBuf::from(OsString::from_vec(path))
                .to_string_lossy()
                .into_owned(),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Runs the aggregate queries and merges the daily rows into the zero-filled
/// calendar. Days absent from the result keep their zero counts.
fn aggregate(
    tx: &Connection,
    ws_id: Option<&String>,
    filter: &Filter,
    start: &str,
    end: &str,
    mut days: Vec<Day>,
) -> Result<(Vec<Day>, Vec<PatternDuration>), StoreError> {
    let (from, until) = bounds(tx, start, end)?;
    let mut statement = tx.prepare(&days_sql())?;
    let found = statement.query_map(
        named_params! {
            ":workspace":ws_id, ":start":from, ":end":until,
            ":first_day":start, ":last_day":end,
            ":cmd":filter.cmd, ":outcome":filter.outcome,
            ":pattern":filter.pattern, ":selected_workspace":filter.workspace_id,
        },
        |r| {
            Ok(Day {
                day: r.get(0)?,
                count: r.get(1)?,
                ok: r.get(2)?,
                failed: r.get(3)?,
                interrupted: r.get(4)?,
                unknown: r.get(5)?,
                timed: r.get(6)?,
                total_ms: r.get(7)?,
                p50_ms: r.get(8)?,
                p95_ms: r.get(9)?,
            })
        },
    )?;
    for row in found {
        let day = row?;
        let i = days
            .binary_search_by(|d| d.day.cmp(&day.day))
            .map_err(|_| super::fail(ErrorKind::Invalid))?;
        days[i] = day;
    }
    let patterns: Vec<PatternDuration> = tx
        .prepare(&patterns_sql())?
        .query_map(
            named_params! {
                ":workspace":ws_id, ":start":from, ":end":until,
                ":first_day":start, ":last_day":end,
                ":cmd":filter.cmd, ":outcome":filter.outcome,
                ":selected_workspace":filter.workspace_id,
            },
            |r| {
                Ok(PatternDuration {
                    pattern: r.get(0)?,
                    invocations: r.get(1)?,
                    total_ms: r.get(2)?,
                    p50_ms: r.get(3)?,
                    p95_ms: r.get(4)?,
                })
            },
        )?
        .collect::<Result<_, _>>()?;
    Ok((days, patterns))
}

const PATTERN_LIMIT: usize = 10;

// Aggregation stays inside SQLite so the full invocation set never enters
// memory; the percentile sorts spill to SQLite-managed storage. `matched`
// is referenced twice, so MATERIALIZED keeps it a single scan. Scalar MAX
// returns NULL on a NULL argument, so `ms` is the duration clamped to zero
// for a backwards clock jump and NULL for an unfinished invocation, making
// COUNT(ms) the timed denominator. An overflowing subtraction promotes to
// REAL and still fails closed when a percentile or total is read back as
// i64. The nearest-rank index ceil(n*p/100) is (n*p+99)/100 in integers,
// and comparisons against a NULL exit code yield NULL, which FILTER skips:
// `exit_code NOT IN (0,8)` leaves only the failed residue. The pattern
// predicate narrows only this daily series; the ranking query below always
// reflects every pattern.
fn days_sql() -> String {
    format!(
        "WITH matched AS MATERIALIZED (
            SELECT date(started_at_ms / 1000,'unixepoch','localtime') AS day,exit_code,
                MAX(finished_at_ms - started_at_ms,0) AS ms
            FROM invocation WHERE {SCOPE} AND {MATCH}
                AND (:pattern IS NULL OR target_pattern=:pattern)
        ),
        daily AS (
            SELECT day,COUNT(*) AS count,
                COUNT(*) FILTER (WHERE exit_code=0) AS ok,
                COUNT(*) FILTER (WHERE exit_code NOT IN (0,8)) AS failed,
                COUNT(*) FILTER (WHERE exit_code=8) AS interrupted,
                COUNT(*) FILTER (WHERE exit_code IS NULL) AS unknown,
                COUNT(ms) AS timed,COALESCE(SUM(ms),0) AS total_ms
            FROM matched GROUP BY day
        ),
        ranked AS (
            SELECT day,
                MAX(CASE WHEN rn=(n*50+99)/100 THEN ms END) AS p50,
                MAX(CASE WHEN rn=(n*95+99)/100 THEN ms END) AS p95
            FROM (
                SELECT day,ms,
                    ROW_NUMBER() OVER (PARTITION BY day ORDER BY ms) AS rn,
                    COUNT(*) OVER (PARTITION BY day) AS n
                FROM matched WHERE ms IS NOT NULL
            ) GROUP BY day
        )
        SELECT daily.day,daily.count,daily.ok,daily.failed,daily.interrupted,
            daily.unknown,daily.timed,daily.total_ms,
            COALESCE(ranked.p50,0),COALESCE(ranked.p95,0)
        FROM daily LEFT JOIN ranked USING(day)"
    )
}

// Daily filtering never removes invocations from the all-pattern ranking:
// this query carries no :pattern predicate at all.
fn patterns_sql() -> String {
    format!(
        "WITH matched AS MATERIALIZED (
            SELECT target_pattern AS pattern,
                MAX(finished_at_ms - started_at_ms,0) AS ms
            FROM invocation WHERE {SCOPE} AND {MATCH}
                AND target_pattern<>''
        ),
        top AS (
            SELECT pattern,COUNT(*) AS invocations,COALESCE(SUM(ms),0) AS total_ms
            FROM matched GROUP BY pattern
            ORDER BY total_ms DESC,pattern LIMIT {PATTERN_LIMIT}
        ),
        ranked AS (
            SELECT pattern,
                MAX(CASE WHEN rn=(n*50+99)/100 THEN ms END) AS p50,
                MAX(CASE WHEN rn=(n*95+99)/100 THEN ms END) AS p95
            FROM (
                SELECT pattern,ms,
                    ROW_NUMBER() OVER (PARTITION BY pattern ORDER BY ms) AS rn,
                    COUNT(*) OVER (PARTITION BY pattern) AS n
                FROM matched WHERE ms IS NOT NULL
            ) GROUP BY pattern
        )
        SELECT top.pattern,top.invocations,top.total_ms,
            COALESCE(ranked.p50,0),COALESCE(ranked.p95,0)
        FROM top LEFT JOIN ranked USING(pattern)
        ORDER BY top.total_ms DESC,top.pattern"
    )
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
            pattern: &str,
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
                    "INSERT INTO invocation(id,workspace_id,cmd,target_pattern,started_at_ms,finished_at_ms,exit_code)
                VALUES(?1,?2,?3,?4,unixepoch(?5,'utc')*1000,unixepoch(?6,'utc')*1000,?7)",
                    params![id, ws_id, cmd, pattern, started, finished, code],
                )
                .unwrap();
            id
        }
        fn set_started_at(&self, id: &str, millis: i64) {
            Store::open(&self.data)
                .unwrap()
                .conn
                .execute(
                    "UPDATE invocation SET started_at_ms=?1 WHERE id=?2",
                    params![millis, id],
                )
                .unwrap();
        }
        fn extra(&self, name: &str) -> PathBuf {
            let ws = self.root.join(name);
            fs::create_dir(&ws).unwrap();
            self.register(&ws);
            ws
        }
        fn ws_id(&self, ws: &Path) -> String {
            Store::open(&self.data)
                .unwrap()
                .conn
                .query_row(
                    "SELECT id FROM workspace WHERE canonical_path=?1",
                    [ws.canonicalize().unwrap().as_os_str().as_encoded_bytes()],
                    |r| r.get(0),
                )
                .unwrap()
        }
        fn set_repo_url(&self, ws: &Path, repo_url: Option<&str>) {
            Store::open(&self.data)
                .unwrap()
                .conn
                .execute(
                    "UPDATE workspace SET repo_url=?1 WHERE canonical_path=?2",
                    params![
                        repo_url,
                        ws.canonicalize().unwrap().as_os_str().as_encoded_bytes()
                    ],
                )
                .unwrap();
        }
        fn insert_ms(
            &self,
            ws: &Path,
            started_ms: i64,
            finished_ms: Option<i64>,
            code: Option<i32>,
            pattern: &str,
        ) -> String {
            let ws_id = self.ws_id(ws);
            let store = Store::open(&self.data).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            store
                .conn
                .execute(
                    "INSERT INTO invocation(id,workspace_id,cmd,target_pattern,started_at_ms,finished_at_ms,exit_code)
                VALUES(?1,?2,'build',?3,?4,?5,?6)",
                    params![id, ws_id, pattern, started_ms, finished_ms, code],
                )
                .unwrap();
            id
        }
        fn noon_ms(&self) -> i64 {
            Store::open(&self.data)
                .unwrap()
                .conn
                .query_row(
                    "SELECT unixepoch('2026-03-08 12:00:00','utc')*1000",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
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
        Store::open(&f.data).unwrap().conn.execute("INSERT INTO invocation(id,workspace_id,cmd,target_pattern,started_at_ms,exit_code) VALUES(?1,?2,'build','//:other',unixepoch('2026-03-07','utc')*1000,0)",params![uuid::Uuid::new_v4().to_string(),other_id]).unwrap();
        let all = f.query(f.filter());
        assert_eq!(
            all.days.iter().map(|d| d.count).collect::<Vec<_>>(),
            vec![3, 0, 1]
        );
        assert_eq!(total(&all), 4);
        assert_eq!(all.days[0].unknown, 1);
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
    fn all_commands_and_unknown_outcomes_are_counted_without_a_pattern() {
        let f = Fixture::new();
        f.insert("2026-03-07 10:00:00", "run", Some(0));
        let pending = f.insert("2026-03-07 11:00:00", "coverage", None);
        Store::open(&f.data)
            .unwrap()
            .conn
            .execute(
                "UPDATE invocation SET target_pattern=NULL WHERE id=?1",
                [&pending],
            )
            .unwrap();

        let all = f.query(f.filter());
        assert_eq!(
            (all.days[0].count, all.days[0].ok, all.days[0].unknown),
            (2, 1, 1)
        );
        assert_eq!(all.patterns.len(), 1);
        let unknown = f.query(Filter {
            outcome: Some("unknown".into()),
            ..f.filter()
        });
        assert_eq!(total(&unknown), 1);
        assert!(unknown.patterns.is_empty());
        let coverage = f.query(Filter {
            cmd: Some("coverage".into()),
            ..f.filter()
        });
        assert_eq!(total(&coverage), 1);
        let run = f.query(Filter {
            cmd: Some("run".into()),
            ..f.filter()
        });
        assert_eq!(total(&run), 1);
    }

    #[test]
    fn pattern_filter_matches_the_complete_list() {
        let f = Fixture::new();
        f.insert_at(
            "2026-03-07 10:00:00",
            "build",
            Some(0),
            "//a/... //b/...",
            None,
        );
        let partial = f.query(Filter {
            pattern: Some("//a/...".into()),
            ..f.filter()
        });
        assert_eq!(total(&partial), 0);
        let whole = f.query(Filter {
            pattern: Some("//a/... //b/...".into()),
            ..f.filter()
        });
        assert_eq!(total(&whole), 1);
    }
    #[test]
    fn daily_duration_statistics_use_only_observed_invocations() {
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
        assert_eq!(first.count, 4);
        assert_eq!((first.ok, first.failed, first.interrupted), (2, 1, 0));
        assert_eq!(first.unknown, 1);
        assert_eq!(first.timed, 3);
        assert_eq!(first.total_ms, 60_000);
        assert_eq!((first.p50_ms, first.p95_ms), (20_000, 30_000));
    }

    #[test]
    fn untimed_invocations_keep_outcome_counts_without_duration_statistics() {
        let f = Fixture::new();
        f.insert_at("2026-03-08 12:00:00", "build", Some(8), "//a:x", None);

        let result = f.query(f.filter());
        let second = &result.days[1];
        assert_eq!((second.count, second.interrupted), (1, 1));
        assert_eq!((second.timed, second.total_ms), (0, 0));
        assert_eq!((second.p50_ms, second.p95_ms), (0, 0));
    }

    #[test]
    fn pattern_ranking_aggregates_invocations_across_days() {
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
        assert_eq!(result.patterns.len(), 3);
        assert_eq!(result.patterns[0].pattern, "//a:x");
        assert_eq!(result.patterns[0].invocations, 3);
        assert_eq!(result.patterns[0].total_ms, 40_000);
        assert_eq!(result.patterns[0].p50_ms, 10_000);
        assert_eq!(result.patterns[1].pattern, "//b:y");
        assert_eq!(result.patterns[1].invocations, 1);
    }

    #[test]
    fn pattern_filter_narrows_daily_series_but_not_ranking() {
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
            pattern: Some("//a:x".into()),
            ..f.filter()
        });
        assert_eq!(scoped.days[0].count, 2);
        assert_eq!(scoped.days[0].total_ms, 40_000);
        assert_eq!(scoped.days[1].count, 1);
        assert_eq!(scoped.patterns.len(), 2);
    }

    #[test]
    fn outcome_filter_narrows_pattern_ranking() {
        let f = Fixture::new();
        f.insert_at("2026-03-07 10:00:00", "build", Some(0), "//a:x", None);
        f.insert_at("2026-03-08 12:00:00", "build", Some(8), "//a:x", None);
        f.insert_at("2026-03-07 12:00:00", "test", Some(1), "//b:y", None);

        let failed = f.query(Filter {
            outcome: Some("failed".into()),
            ..f.filter()
        });
        assert_eq!(failed.patterns.len(), 1);
        assert_eq!(failed.patterns[0].pattern, "//b:y");
    }

    #[test]
    fn oversized_pattern_filter_is_rejected() {
        let f = Fixture::new();
        assert_eq!(
            query(
                &f.data,
                &f.ws,
                Filter {
                    pattern: Some("x".repeat(64 * 1024 + 1)),
                    ..f.filter()
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::Invalid
        );
    }
    #[test]
    fn pattern_ranking_is_ordered_and_truncated() {
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
        // One heavier invocation and a same-total pair to pin the label
        // tie-break.
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
        assert_eq!(result.patterns.len(), PATTERN_LIMIT);
        assert_eq!(result.patterns[0].pattern, "//t:heavy");
        // Every 1s target ties; the byte order of the label breaks the tie.
        let tied: Vec<&str> = result.patterns[1..]
            .iter()
            .map(|t| t.pattern.as_str())
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
    fn default_filter_spans_thirty_days_ending_today() {
        let f = Fixture::new();
        let result = f.query(Filter::default());
        assert_eq!(result.days.len(), 30);
        assert_eq!(total(&result), 0);
        assert_eq!(result.filter.end, Some(result.today));
    }
    #[test]
    fn unregistered_workspaces_have_empty_analytics() {
        let f = Fixture::new();
        let other = f.root.join("unregistered");
        fs::create_dir(&other).unwrap();
        assert_eq!(total(&query(&f.data, &other, f.filter()).unwrap()), 0);
    }
    #[test]
    fn workspace_aliases_resolve_to_the_same_history() {
        let f = Fixture::new();
        let alias = f.root.join("alias");
        std::os::unix::fs::symlink(&f.ws, &alias).unwrap();
        f.insert("2026-03-07", "build", Some(0));
        assert_eq!(total(&query(&f.data, &alias, f.filter()).unwrap()), 1);
    }
    #[test]
    fn invalid_bound_values_are_rejected() {
        let f = Fixture::new();
        f.insert("2026-03-07", "build", Some(0));
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
        let (from, until): (i64, i64) = Store::open(&f.data)
            .unwrap()
            .conn
            .query_row(
                "SELECT unixepoch('2026-11-01 04:00:00')*1000,unixepoch('2026-11-02 05:00:00')*1000",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
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
                "UPDATE invocation SET started_at_ms=unixepoch('1988-10-30 02:00:30')*1000 WHERE id=?1",
                [early],
            )
            .unwrap();
        Store::open(&f.data)
            .unwrap()
            .conn
            .execute(
                "UPDATE invocation SET started_at_ms=unixepoch('1988-10-30 03:00:00')*1000 WHERE id=?1",
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
    const SHARED_URL: &str = "git@example.com:org/shared.git";
    const OTHER_URL: &str = "git@example.com:org/other.git";

    fn grouped() -> Fixture {
        let f = Fixture::new();
        let second = f.extra("second");
        let third = f.extra("third");
        let other = f.extra("other-repo");
        let plain = f.extra("plain");
        f.set_repo_url(&f.ws, Some(SHARED_URL));
        f.set_repo_url(&second, Some(SHARED_URL));
        f.set_repo_url(&third, Some(SHARED_URL));
        f.set_repo_url(&other, Some(OTHER_URL));
        let start = f.noon_ms();
        f.insert_ms(&f.ws, start, Some(start + 100), Some(0), "//a:x");
        f.insert_ms(&second, start, Some(start + 200), Some(1), "//b:y");
        f.insert_ms(&third, start, Some(start + 900), Some(0), "//a:x");
        f.insert_ms(&other, start, Some(start + 5000), Some(0), "//d:x");
        f.insert_ms(&plain, start, Some(start + 7000), Some(0), "//n:x");
        f
    }

    fn assert_invalid_selection(f: &Fixture, workspace_id: String) {
        assert_eq!(
            query(
                &f.data,
                &f.ws,
                Filter {
                    workspace_id: Some(workspace_id),
                    ..f.filter()
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::Invalid
        );
    }

    #[test]
    fn repo_url_groups_the_candidate_workspaces() {
        let f = grouped();
        let result = f.query(f.filter());
        let second = f.root.join("second");
        let third = f.root.join("third");
        let mut expected: Vec<(String, String)> = [&f.ws, &second, &third]
            .into_iter()
            .map(|ws| {
                (
                    f.ws_id(ws),
                    ws.canonicalize().unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect();
        expected.sort();
        let mut actual: Vec<(String, String)> = result
            .workspaces
            .iter()
            .map(|w| (w.id.clone(), w.path.clone()))
            .collect();
        actual.sort();
        assert_eq!(actual, expected);
    }

    #[test]
    fn repo_url_scopes_daily_aggregates() {
        let f = grouped();
        let result = f.query(f.filter());
        assert_eq!(total(&result), 3);
        let days: Vec<_> = result.days.iter().filter(|day| day.count > 0).collect();
        assert_eq!(days.len(), 1);
        let day = days[0];
        assert_eq!(
            (
                day.count,
                day.timed,
                day.total_ms,
                day.p50_ms,
                day.p95_ms,
                day.ok,
                day.failed,
                day.interrupted,
                day.unknown
            ),
            (3, 3, 1200, 200, 900, 2, 1, 0, 0)
        );
    }

    #[test]
    fn repo_url_scopes_pattern_aggregates() {
        let f = grouped();
        let result = f.query(f.filter());
        let patterns: Vec<(&str, i64, i64, i64, i64)> = result
            .patterns
            .iter()
            .map(|p| {
                (
                    p.pattern.as_str(),
                    p.invocations,
                    p.total_ms,
                    p.p50_ms,
                    p.p95_ms,
                )
            })
            .collect();
        assert_eq!(
            patterns,
            [("//a:x", 2, 1000, 100, 900), ("//b:y", 1, 200, 200, 200)]
        );
    }

    #[test]
    fn selected_workspace_limits_scope_and_rejects_outsiders() {
        let f = grouped();
        let id_b = f.ws_id(&f.root.join("second"));

        let selected = f.query(Filter {
            workspace_id: Some(id_b.clone()),
            ..f.filter()
        });
        assert_eq!(total(&selected), 1);
        let day = selected.days.iter().find(|day| day.count > 0).unwrap();
        assert_eq!(
            (day.count, day.timed, day.total_ms, day.p50_ms, day.p95_ms),
            (1, 1, 200, 200, 200)
        );
        assert_eq!(selected.patterns.len(), 1);
        assert_eq!(selected.patterns[0].pattern, "//b:y");
        assert_eq!(selected.workspaces.len(), 3);

        let narrow = f.query(Filter {
            cmd: Some("nonexistent".into()),
            workspace_id: Some(id_b),
            ..f.filter()
        });
        assert_eq!(narrow.workspaces.len(), 3);
        assert_eq!(total(&narrow), 0);

        assert_invalid_selection(&f, f.ws_id(&f.root.join("other-repo")));
        assert_invalid_selection(&f, uuid::Uuid::new_v4().to_string());
    }

    #[test]
    fn workspaces_without_repo_url_only_scope_to_themselves() {
        let f = Fixture::new();
        let other = f.extra("other-plain");
        let id_a = f.ws_id(&f.ws);
        let id_b = f.ws_id(&other);

        let start = f.noon_ms();
        f.insert_ms(&f.ws, start, Some(start + 100), Some(0), "//a:x");
        f.insert_ms(&other, start, Some(start + 900), Some(0), "//b:y");

        let result = f.query(f.filter());
        assert_eq!(result.workspaces.len(), 1);
        assert_eq!(result.workspaces[0].id, id_a);
        assert_eq!(total(&result), 1);

        let from_other = query(&f.data, &other, f.filter()).unwrap();
        assert_eq!(from_other.workspaces.len(), 1);
        assert_eq!(from_other.workspaces[0].id, id_b);
        assert_eq!(total(&from_other), 1);
        let day = from_other.days.iter().find(|day| day.count > 0).unwrap();
        assert_eq!(day.total_ms, 900);
    }

    #[test]
    fn refresh_workspace_groups_existing_history_without_new_invocations() {
        let f = Fixture::new();
        let other = f.extra("other");
        let start = f.noon_ms();
        f.insert_ms(&f.ws, start, Some(start + 100), Some(0), "//a:x");
        f.insert_ms(&other, start, Some(start + 200), Some(0), "//b:y");
        assert_eq!(total(&f.query(f.filter())), 1);

        crate::workspace::test_support::repo(&f.ws, SHARED_URL);
        crate::workspace::test_support::repo(&other, SHARED_URL);
        crate::store::refresh_workspace(&f.data, &f.ws).unwrap();
        crate::store::refresh_workspace(&f.data, &other).unwrap();

        let result = f.query(f.filter());
        assert_eq!(total(&result), 2);
        assert_eq!(result.workspaces.len(), 2);
        let day = result.days.iter().find(|day| day.count > 0).unwrap();
        assert_eq!((day.count, day.timed, day.total_ms), (2, 2, 300));
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
            tx.execute("INSERT INTO invocation(id,workspace_id,cmd,target_pattern,started_at_ms,exit_code) VALUES(?1,?2,?3,'//synthetic:target',unixepoch('2026-03-01',?4,'utc')*1000,?5)",
                params![uuid::Uuid::new_v4().to_string(),ids[i%2],["build","build","test","test"][i%4],format!("+{} days",i%30),[0,1,8][i%3]]).unwrap();
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
