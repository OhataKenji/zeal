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
                total_ms: 0,
                p50_ms: 0,
                p95_ms: 0,
            })
        })?
        .collect::<Result<_, _>>()?;
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
                Ok(Observation {
                    day: r.get(0)?,
                    exit_code: r.get(1)?,
                    target: r.get(2)?,
                    duration_ms: r.get(3)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let (days, targets) = summarize(days, rows, filter.target.as_deref())?;
    Ok(Analytics {
        filter,
        today,
        days,
        targets,
    })
}

struct Observation {
    day: String,
    exit_code: i64,
    target: String,
    duration_ms: Option<i64>,
}

/// Daily filtering never removes runs from the all-target ranking.
fn summarize(
    mut days: Vec<Day>,
    rows: Vec<Observation>,
    target_filter: Option<&str>,
) -> Result<(Vec<Day>, Vec<TargetDuration>), StoreError> {
    let mut durations: Vec<Vec<i64>> = vec![Vec::new(); days.len()];
    let mut by_target: HashMap<String, TargetAgg> = HashMap::new();
    for Observation {
        day,
        exit_code,
        target,
        duration_ms,
    } in rows
    {
        // A clock jump backwards must not corrupt the series.
        let duration = duration_ms.map(|ms| ms.max(0));
        let entry = by_target.entry(target.clone()).or_default();
        entry.runs += 1;
        if let Some(duration) = duration {
            entry.durations.push(duration);
        }
        if target_filter.is_some_and(|t| t != target) {
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
    Ok((days, targets))
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
mod tests;
