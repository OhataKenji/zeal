//! Read-only projections of completed GUI runs. Artifacts and live runner state
//! never participate in counting or filtering.
use super::{ErrorKind, Store, StoreError, fail};
use rusqlite::{Connection, OptionalExtension, named_params, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Filter {
    pub start: Option<String>,
    pub end: Option<String>,
    pub cmd: Option<String>,
    pub outcome: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Day {
    pub day: String,
    pub count: i64,
}
#[derive(Debug, Serialize)]
pub struct Analytics {
    pub filter: Filter,
    pub today: String,
    pub days: Vec<Day>,
}

// The existing index supplies the workspace + millisecond range scan.
// The local-date predicate determines which calendar days to count.
const MATCH: &str = "workspace_id=:workspace
    AND started_at_ms >= :start AND started_at_ms < :end
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
        return Err(fail(ErrorKind::Invalid));
    }
    let normalized: Option<String> =
        conn.query_row("SELECT date(?1,'+0 days')", [day], |r| r.get(0))?;
    if normalized.as_deref() != Some(day) {
        return Err(fail(ErrorKind::Invalid));
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
    start.zip(end).ok_or_else(|| fail(ErrorKind::Invalid))
}

/// POSIX timezone state is refreshed when opening a workspace. No TZ env or
/// process-global environment mutation is needed by the app.
pub fn refresh_timezone() {
    unsafe extern "C" {
        fn tzset();
    }
    // SAFETY: tzset takes no arguments and initializes libc's timezone tables.
    unsafe {
        tzset();
    }
}

pub fn query(data: &Path, ws: &Path, mut filter: Filter) -> Result<Analytics, StoreError> {
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
    {
        return Err(fail(ErrorKind::Invalid));
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
                count: r.get(1)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    if let Some(ws_id) = ws_id {
        let sql = format!(
            "SELECT date(started_at_ms / 1000,'unixepoch','localtime'),COUNT(*) FROM run WHERE {MATCH} GROUP BY 1 ORDER BY 1"
        );
        let mut statement = tx.prepare(&sql)?;
        let counts = statement
            .query_map(
                named_params! {
                    ":workspace":ws_id, ":start":from, ":end":until,
                    ":first_day":start, ":last_day":end,
                    ":cmd":filter.cmd, ":outcome":filter.outcome,
                },
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        for (day, count) in counts {
            let i = days
                .binary_search_by(|d| d.day.cmp(&day))
                .map_err(|_| fail(ErrorKind::Invalid))?;
            days[i].count = count;
        }
    }
    Ok(Analytics {
        filter,
        today,
        days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, process::Command, time::Instant};
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
            let root =
                std::env::temp_dir().join(format!("zeal-analytics-{}", uuid::Uuid::new_v4()));
            let data = root.join("data");
            let ws = root.join("workspace");
            fs::create_dir_all(&ws).unwrap();
            // Register workspace through M4; this unfinished record is excluded.
            super::super::create(&data, &ws, "build", "//:unfinished").unwrap();
            Self { root, data, ws }
        }
        fn insert(&self, day: &str, cmd: &str, code: Option<i32>) -> String {
            let store = Store::open(&self.data).unwrap();
            let ws_id = store
                .workspace_id(&self.ws.canonicalize().unwrap())
                .unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            store
                .conn
                .execute(
                    "INSERT INTO run(id,workspace_id,cmd,target_raw,started_at_ms,exit_code)
                VALUES(?1,?2,?3,'//synthetic:target',unixepoch(?4,'utc')*1000,?5)",
                    params![id, ws_id, cmd, day, code],
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
