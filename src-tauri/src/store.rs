//! SQLite run summaries. Bazel still writes observations into each run directory.
pub mod analytics;
pub mod events;
#[path = "securefs.rs"]
mod securefs;

use rusqlite::{Connection, OpenFlags, params};
use securefs::Dir;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Corrupt,
    Permission,
    DiskFull,
    Capacity,
    NewerSchema,
    Missing,
    Conflict,
    Invalid,
    Busy,
    Io,
}
#[derive(Debug)]
pub struct StoreError {
    pub kind: ErrorKind,
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            ErrorKind::Corrupt => "history database is corrupt or has an unsupported schema",
            ErrorKind::Permission => "history storage permission or file safety error",
            ErrorKind::DiskFull => "history storage is full",
            ErrorKind::Capacity => "BES result display limit reached",
            ErrorKind::NewerSchema => "history database requires a newer Zeal version",
            ErrorKind::Missing => "history record or artifact is missing",
            ErrorKind::Conflict => "history record or directory already exists",
            ErrorKind::Invalid => "invalid history input",
            ErrorKind::Busy => "history database is busy",
            ErrorKind::Io => "history storage I/O error",
        })
    }
}
impl std::error::Error for StoreError {}
impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        let kind = match error.raw_os_error() {
            Some(libc::ENOSPC | libc::EDQUOT) => ErrorKind::DiskFull,
            Some(libc::ELOOP | libc::ENOTDIR) => ErrorKind::Permission,
            _ => match error.kind() {
                io::ErrorKind::PermissionDenied => ErrorKind::Permission,
                io::ErrorKind::NotFound => ErrorKind::Missing,
                io::ErrorKind::AlreadyExists => ErrorKind::Conflict,
                io::ErrorKind::InvalidInput => ErrorKind::Invalid,
                _ => ErrorKind::Io,
            },
        };
        Self { kind }
    }
}
impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode;
        Self {
            kind: match error {
                rusqlite::Error::SqliteFailure(error, _) => match error.code {
                    ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => ErrorKind::Corrupt,
                    ErrorCode::PermissionDenied | ErrorCode::ReadOnly | ErrorCode::CannotOpen => {
                        ErrorKind::Permission
                    }
                    ErrorCode::DiskFull => ErrorKind::DiskFull,
                    ErrorCode::ConstraintViolation => ErrorKind::Conflict,
                    ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => ErrorKind::Busy,
                    _ => ErrorKind::Io,
                },
                rusqlite::Error::QueryReturnedNoRows => ErrorKind::Missing,
                _ => ErrorKind::Corrupt,
            },
        }
    }
}
impl From<StoreError> for io::Error {
    fn from(error: StoreError) -> Self {
        let kind = match error.kind {
            ErrorKind::Missing => io::ErrorKind::NotFound,
            ErrorKind::Permission => io::ErrorKind::PermissionDenied,
            ErrorKind::Invalid => io::ErrorKind::InvalidInput,
            _ => io::ErrorKind::Other,
        };
        io::Error::new(kind, error)
    }
}
fn fail(kind: ErrorKind) -> StoreError {
    StoreError { kind }
}

/// The existing UI record contract. Status remains derived from end and exit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub workspace: String,
    pub cmd: String,
    pub target: String,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external: bool,
}

fn id(value: &str) -> Result<String, StoreError> {
    uuid::Uuid::parse_str(value)
        .map(|value| value.to_string())
        .map_err(|_| fail(ErrorKind::Invalid))
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_millis() as i64)
        .unwrap_or(0)
}

struct Store {
    conn: Connection,
    root: Dir,
    // Keep SQLite close/WAL deletion inside the same lock as file checks.
    _access: std::sync::MutexGuard<'static, ()>,
}
impl Store {
    fn open(data: &Path) -> Result<Self, StoreError> {
        static ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let access = ACCESS.lock().unwrap();
        let root = Dir::root(data)?;
        let exists = root.exists("zeal.db")?;
        for name in ["zeal.db-wal", "zeal.db-shm", "zeal.db-journal"] {
            if root.exists(name)? {
                if !exists {
                    return Err(fail(ErrorKind::Corrupt));
                }
                root.file(name, false)?;
            }
        }
        let db = root.file("zeal.db", !exists)?;
        if exists && db.metadata()?.len() == 0 {
            return Err(fail(ErrorKind::Corrupt));
        }
        db.sync_all()?;
        root.sync()?;
        let mut conn = Connection::open_with_flags(
            root.path().join("zeal.db"),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 2 {
            return Err(fail(ErrorKind::NewerSchema));
        }
        if exists && ![1, 2].contains(&version) {
            return Err(fail(ErrorKind::Corrupt));
        }
        let journal: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        if journal != "wal" {
            return Err(fail(ErrorKind::Io));
        }
        conn.execute_batch("PRAGMA synchronous=FULL;")?;
        if !exists {
            let tx = conn.transaction()?;
            tx.execute_batch("CREATE TABLE workspace(id TEXT PRIMARY KEY, canonical_path BLOB NOT NULL UNIQUE);
                CREATE TABLE run(id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL REFERENCES workspace(id),
                    cmd TEXT NOT NULL,target_raw TEXT NOT NULL,started_at_ms INTEGER NOT NULL,finished_at_ms INTEGER,exit_code INTEGER);
                CREATE INDEX run_workspace_started ON run(workspace_id,started_at_ms DESC);
                PRAGMA user_version=1;")?;
            tx.commit()?;
        }
        if version < 2 {
            let tx = conn.transaction()?;
            tx.execute_batch(events::SCHEMA)?;
            tx.commit()?;
        }
        conn.prepare("SELECT id,workspace_id,cmd,target_raw,started_at_ms,finished_at_ms,exit_code FROM run LIMIT 0")
            .map_err(|_| fail(ErrorKind::Corrupt))?;
        for name in ["zeal.db-wal", "zeal.db-shm", "zeal.db-journal"] {
            if root.exists(name)? {
                root.file(name, false)?;
            }
        }
        Ok(Self {
            conn,
            root,
            _access: access,
        })
    }
    fn workspace_id(&self, ws: &Path) -> Result<String, StoreError> {
        Ok(self.conn.query_row(
            "SELECT id FROM workspace WHERE canonical_path=?1",
            [ws.as_os_str().as_bytes()],
            |row| row.get(0),
        )?)
    }
    fn runs(&self, ws_id: &str, create: bool) -> Result<Dir, StoreError> {
        Ok(self
            .root
            .child("workspaces", create)?
            .child(&id(ws_id)?, create)?
            .child("runs", create)?)
    }
}
const SELECT: &str = "SELECT r.id,r.cmd,r.target_raw,r.started_at_ms,r.finished_at_ms,r.exit_code FROM run r JOIN workspace w ON w.id=r.workspace_id WHERE w.canonical_path=?1";
fn row_record(row: &rusqlite::Row<'_>, ws: &Path) -> rusqlite::Result<RunRecord> {
    let started: i64 = row.get(3)?;
    let finished: Option<i64> = row.get(4)?;
    Ok(RunRecord {
        id: row.get(0)?,
        workspace: ws.to_string_lossy().into_owned(),
        cmd: row.get(1)?,
        target: row.get(2)?,
        started_at: started
            .try_into()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, started))?,
        finished_at: finished
            .map(|value| {
                value
                    .try_into()
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(4, value))
            })
            .transpose()?,
        exit_code: row.get(5)?,
        external: false,
    })
}

pub fn create(
    data: &Path,
    ws: &Path,
    cmd: &str,
    target: &str,
) -> Result<(RunRecord, PathBuf), StoreError> {
    let ws = ws.canonicalize()?;
    let mut store = Store::open(data)?;
    let proposed = uuid::Uuid::new_v4().to_string();
    store.conn.execute("INSERT INTO workspace(id,canonical_path) VALUES(?1,?2) ON CONFLICT(canonical_path) DO NOTHING",params![proposed,ws.as_os_str().as_bytes()])?;
    let ws_id = store.workspace_id(&ws)?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let runs = store.runs(&ws_id, true)?;
    let dir = runs.create_child(&run_id)?;
    dir.file("log.txt", true)?.sync_all()?;
    dir.sync()?;
    let started = now_ms();
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO run(id,workspace_id,cmd,target_raw,started_at_ms) VALUES(?1,?2,?3,?4,?5)",
        params![run_id, ws_id, cmd, target, started],
    )?;
    tx.commit()?;
    let rec = RunRecord {
        id: run_id,
        workspace: ws.to_string_lossy().into_owned(),
        cmd: cmd.into(),
        target: target.into(),
        started_at: started as u64,
        finished_at: None,
        exit_code: None,
        external: false,
    };
    Ok((rec, dir.path().to_owned()))
}
pub fn read_record(data: &Path, ws: &Path, run_id: &str) -> Result<RunRecord, StoreError> {
    let ws = ws.canonicalize()?;
    let store = Store::open(data)?;
    let result = store.conn.query_row(
        &format!("{SELECT} AND r.id=?2"),
        params![ws.as_os_str().as_bytes(), id(run_id)?],
        |row| row_record(row, &ws),
    );
    match result {
        Ok(record) => Ok(record),
        Err(rusqlite::Error::QueryReturnedNoRows) => events::read(&store, &ws, run_id)?
            .map(|observation| observation.record())
            .ok_or_else(|| fail(ErrorKind::Missing)),
        Err(error) => Err(error.into()),
    }
}
pub fn list(data: &Path, ws: &Path) -> Result<Vec<RunRecord>, StoreError> {
    let ws = ws.canonicalize()?;
    let store = Store::open(data)?;
    let mut statement = store
        .conn
        .prepare(&format!("{SELECT} ORDER BY r.started_at_ms DESC,r.id"))?;
    let mut records = statement
        .query_map([ws.as_os_str().as_bytes()], |row| row_record(row, &ws))?
        .collect::<Result<Vec<_>, _>>()?;
    for observation in events::list(&store, &ws)? {
        if !records.iter().any(|record| record.id == observation.id) {
            records.push(observation);
        }
    }
    records.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(a.id.cmp(&b.id)));
    Ok(records)
}
/// Only an unfinished record can transition to a completed summary.
pub fn finish(
    data: &Path,
    ws: &Path,
    run_id: &str,
    exit_code: i32,
) -> Result<RunRecord, StoreError> {
    let ws = ws.canonicalize()?;
    let mut store = Store::open(data)?;
    let run_id = id(run_id)?;
    let tx = store.conn.transaction()?;
    tx.execute("UPDATE run SET finished_at_ms=?1,exit_code=?2 WHERE id=?3 AND finished_at_ms IS NULL AND workspace_id=(SELECT id FROM workspace WHERE canonical_path=?4)",params![now_ms(),exit_code,run_id,ws.as_os_str().as_bytes()])?;
    let rec = tx.query_row(
        &format!("{SELECT} AND r.id=?2"),
        params![ws.as_os_str().as_bytes(), run_id],
        |row| row_record(row, &ws),
    )?;
    tx.commit()?;
    Ok(rec)
}
fn directory(data: &Path, ws: &Path, run_id: &str) -> Result<Dir, StoreError> {
    let ws = ws.canonicalize()?;
    let store = Store::open(data)?;
    let run_id = id(run_id)?;
    let ws_id: String = store.conn.query_row(
        "SELECT r.workspace_id FROM run r JOIN workspace w ON w.id=r.workspace_id WHERE w.canonical_path=?1 AND r.id=?2",
        params![ws.as_os_str().as_bytes(),run_id],|row| row.get(0))?;
    Ok(store.runs(&ws_id, false)?.child(&run_id, false)?)
}
pub fn artifact(data: &Path, ws: &Path, run_id: &str, name: &str) -> io::Result<File> {
    if !["log.txt", "bep.json", "profile.gz"].contains(&name) {
        return Err(fail(ErrorKind::Invalid).into());
    }
    directory(data, ws, run_id)?
        .artifact(name)
        .map_err(StoreError::from)
        .map_err(io::Error::from)
}
/// Spawn failed before a child existed. Remove only this unfinished run's known
/// files; unexpected contents are left in place and reported, never recursed.
pub fn abort_create(data: &Path, ws: &Path, run_id: &str) -> Result<(), StoreError> {
    let ws = ws.canonicalize()?;
    let mut store = Store::open(data)?;
    let ws_id = store.workspace_id(&ws)?;
    let run_id = id(run_id)?;
    let runs = store.runs(&ws_id, false)?;
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let unfinished = tx.query_row(
        "SELECT finished_at_ms IS NULL FROM run WHERE id=?1 AND workspace_id=?2",
        params![run_id, ws_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !unfinished {
        return Err(fail(ErrorKind::Conflict));
    }
    match runs.child(&run_id, false) {
        Ok(dir) => {
            for name in ["log.txt", "bep.json", "profile.gz"] {
                if dir.exists(name)? {
                    dir.remove_file(name)?;
                }
            }
            runs.remove_dir(&run_id)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    tx.execute(
        "DELETE FROM run WHERE id=?1 AND workspace_id=?2 AND finished_at_ms IS NULL",
        params![run_id, ws_id],
    )?;
    tx.commit()?;
    Ok(())
}
#[cfg(test)]
pub fn read_log(dir: &Path) -> io::Result<String> {
    std::fs::read(dir.join("log.txt")).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
pub fn append_log(dir: &Path, line: &str) -> io::Result<()> {
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("log.txt"))?,
        "{line}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    struct Fixture {
        root: PathBuf,
        data: PathBuf,
        ws: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-store-{}", uuid::Uuid::new_v4()));
            let data = root.join("data");
            let ws = root.join("workspace");
            fs::create_dir_all(&ws).unwrap();
            Self { root, data, ws }
        }
        fn create(&self) -> (RunRecord, PathBuf) {
            create(&self.data, &self.ws, "build", "//pkg:all").unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn concurrent_reads_keep_sqlite_sidecars_safe_through_close() {
        let f = Fixture::new();
        let (record, _) = f.create();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let f = &f;
                let record = &record;
                scope.spawn(move || {
                    for _ in 0..25 {
                        assert_eq!(list(&f.data, &f.ws).unwrap(), vec![record.clone()]);
                    }
                });
            }
        });
    }

    #[test]
    fn create_finish_reopen_preserves_original_dto_and_private_files() {
        let f = Fixture::new();
        let (rec, dir) = f.create();
        append_log(&dir, "output").unwrap();
        assert_eq!((rec.finished_at, rec.exit_code), (None, None));
        let done = finish(&f.data, &f.ws, &rec.id, 7).unwrap();
        assert_eq!(done.exit_code, Some(7));
        assert!(done.finished_at.is_some());
        assert_eq!(read_record(&f.data, &f.ws, &rec.id).unwrap(), done);
        assert_eq!(list(&f.data, &f.ws).unwrap(), vec![done.clone()]);
        assert_eq!(read_log(&dir).unwrap(), "output\n");
        assert_eq!(
            serde_json::to_value(&done)
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            7
        );
        assert_eq!(
            fs::metadata(f.data.join("zeal.db"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(dir.join("log.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let store = Store::open(&f.data).unwrap();
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA synchronous", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
    }
    #[test]
    fn workspace_membership_guards_every_read_update_and_abort() {
        let f = Fixture::new();
        let (rec, _) = f.create();
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        assert!(list(&f.data, &other).unwrap().is_empty());
        assert_eq!(
            read_record(&f.data, &other, &rec.id).unwrap_err().kind,
            ErrorKind::Missing
        );
        assert!(finish(&f.data, &other, &rec.id, 0).is_err());
        assert!(abort_create(&f.data, &other, &rec.id).is_err());
        assert_eq!(
            artifact(&f.data, &other, &rec.id, "log.txt")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(read_record(&f.data, &f.ws, &rec.id).unwrap(), rec);
    }
    #[test]
    fn completed_summary_is_immutable_when_finished_twice_or_details_deleted() {
        let f = Fixture::new();
        let (rec, dir) = f.create();
        let done = finish(&f.data, &f.ws, &rec.id, 0).unwrap();
        assert_eq!(finish(&f.data, &f.ws, &rec.id, 9).unwrap(), done);
        assert_eq!(
            abort_create(&f.data, &f.ws, &rec.id).unwrap_err().kind,
            ErrorKind::Conflict
        );
        fs::remove_dir_all(dir).unwrap();
        assert!(artifact(&f.data, &f.ws, &rec.id, "log.txt").is_err());
        assert_eq!(list(&f.data, &f.ws).unwrap(), vec![done]);
    }
    #[test]
    fn unfinished_runs_survive_reopen_and_list_is_newest_first() {
        let f = Fixture::new();
        let (old, _) = f.create();
        let (new, _) = f.create();
        let store = Store::open(&f.data).unwrap();
        store
            .conn
            .execute("UPDATE run SET started_at_ms=1 WHERE id=?1", [&old.id])
            .unwrap();
        store
            .conn
            .execute("UPDATE run SET started_at_ms=2 WHERE id=?1", [&new.id])
            .unwrap();
        drop(store);
        let records = list(&f.data, &f.ws).unwrap();
        assert_eq!(records[0].id, new.id);
        assert_eq!(records[1].id, old.id);
        assert!(records[0].finished_at.is_none());
    }
    #[test]
    fn spawn_failure_removes_only_the_unfinished_row_and_known_files() {
        let f = Fixture::new();
        let (rec, dir) = f.create();
        abort_create(&f.data, &f.ws, &rec.id).unwrap();
        assert!(!dir.exists());
        assert!(list(&f.data, &f.ws).unwrap().is_empty());
        let (rec, dir) = f.create();
        fs::write(dir.join("unexpected"), b"keep").unwrap();
        assert!(abort_create(&f.data, &f.ws, &rec.id).is_err());
        assert!(dir.join("unexpected").exists());
        assert!(read_record(&f.data, &f.ws, &rec.id).is_ok());
    }
    #[test]
    fn canonical_workspace_alias_uses_one_identity_and_missing_roots_are_rejected() {
        let f = Fixture::new();
        let (rec, _) = f.create();
        let alias = f.root.join("alias");
        symlink(&f.ws, &alias).unwrap();
        assert_eq!(read_record(&f.data, &alias, &rec.id).unwrap(), rec);
        assert!(list(&f.data, &f.root.join("missing")).is_err());
    }
    #[test]
    fn unsafe_artifact_names_links_and_preexisting_run_dirs_are_not_used() {
        let f = Fixture::new();
        let (rec, dir) = f.create();
        assert!(artifact(&f.data, &f.ws, "../../escape", "log.txt").is_err());
        assert!(artifact(&f.data, &f.ws, &rec.id, "../zeal.db").is_err());
        fs::remove_file(dir.join("log.txt")).unwrap();
        symlink(f.data.join("zeal.db"), dir.join("log.txt")).unwrap();
        assert_eq!(
            artifact(&f.data, &f.ws, &rec.id, "log.txt")
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        let store = Store::open(&f.data).unwrap();
        let runs = store
            .runs(
                &store.workspace_id(&f.ws.canonicalize().unwrap()).unwrap(),
                false,
            )
            .unwrap();
        assert_eq!(
            runs.create_child(&rec.id).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }
    #[test]
    fn sidecar_only_and_bad_database_refuse_automatic_initialization() {
        let f = Fixture::new();
        let root = Dir::root(&f.data).unwrap();
        root.file("zeal.db-wal", true).unwrap();
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
        assert!(!f.data.join("zeal.db").exists());
        root.remove_file("zeal.db-wal").unwrap();
        root.file("zeal.db", true).unwrap();
        fs::write(f.data.join("zeal.db"), b"invalid").unwrap();
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
    }
    #[test]
    fn newer_schema_and_database_sidecar_links_are_rejected() {
        let f = Fixture::new();
        let store = Store::open(&f.data).unwrap();
        store.conn.pragma_update(None, "user_version", 3).unwrap();
        drop(store);
        assert_eq!(
            Store::open(&f.data).err().unwrap().kind,
            ErrorKind::NewerSchema
        );
        symlink(f.data.join("zeal.db"), f.data.join("zeal.db-wal")).unwrap();
        assert_eq!(
            Store::open(&f.data).err().unwrap().kind,
            ErrorKind::Permission
        );
    }
}
