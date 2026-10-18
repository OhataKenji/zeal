//! Immutable, workspace-scoped invocation summaries and private artifacts.
//! Log placement is durable before the single summary INSERT transaction.
#[path = "securefs.rs"]
mod securefs;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use securefs::Dir;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

pub const MAX_LOG_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROFILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TIME: u64 = 253_402_300_799_999;
const MAX_LIST_ROWS: usize = 10_000;
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Corrupt,
    Permission,
    DiskFull,
    NewerSchema,
    Missing,
    Conflict,
    Invalid,
    TooLarge,
    Busy,
    Io,
}
#[derive(Debug)]
pub struct StoreError {
    pub kind: ErrorKind,
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            ErrorKind::Corrupt => {
                "history database is corrupt; stop all Zeal and Bazel processes before manually backing up the database and sidecars"
            }
            ErrorKind::Permission => "history storage permission or file safety error",
            ErrorKind::DiskFull => "history storage is full",
            ErrorKind::NewerSchema => "history database requires a newer Zeal version",
            ErrorKind::Missing => "history record or artifact is missing",
            ErrorKind::Conflict => "history invocation already exists",
            ErrorKind::Invalid => "invalid history input",
            ErrorKind::TooLarge => "history data exceeds the read limit",
            ErrorKind::Busy => "history database is busy",
            ErrorKind::Io => "history storage I/O error",
        };
        f.write_str(message)
    }
}
impl std::error::Error for StoreError {}
impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        let kind = match e.raw_os_error() {
            Some(libc::ENOSPC | libc::EDQUOT) => ErrorKind::DiskFull,
            Some(libc::ELOOP | libc::ENOTDIR) => ErrorKind::Permission,
            _ => match e.kind() {
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
    fn from(e: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode;
        let kind = match e {
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
        };
        Self { kind }
    }
}
fn fail(kind: ErrorKind) -> StoreError {
    StoreError { kind }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub workspace: String,
    pub workspace_id: String,
    pub cmd: String,
    pub target: String,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub bazel_elapsed_ms: Option<u64>,
    pub outcome: String,
    pub log_truncated: bool,
}

fn uuid(value: &str) -> Result<String, StoreError> {
    uuid::Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| fail(ErrorKind::Invalid))
}
fn canonical(ws: &Path) -> Result<PathBuf, StoreError> {
    Ok(ws.canonicalize()?)
}

struct Store {
    conn: Connection,
    root: Dir,
}
impl Store {
    fn open(data: &Path) -> Result<Self, StoreError> {
        let root = Dir::root(data)?;
        let exists = root.exists("zeal.db")?;
        if !exists
            && ["zeal.db-wal", "zeal.db-shm", "zeal.db-journal"]
                .iter()
                .any(|name| root.exists(name).unwrap_or(true))
        {
            return Err(fail(ErrorKind::Corrupt));
        }
        validate_sidecars(&root)?;
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
        if version > SCHEMA_VERSION {
            return Err(fail(ErrorKind::NewerSchema));
        }
        if exists && version != SCHEMA_VERSION {
            return Err(fail(ErrorKind::Corrupt));
        }
        // SQLite creates WAL/SHM with the already protected database's 0600 mode.
        let journal: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        if journal != "wal" {
            return Err(fail(ErrorKind::Io));
        }
        conn.execute_batch("PRAGMA synchronous=FULL;")?;
        if !exists {
            let tx = conn.transaction()?;
            tx.execute_batch("CREATE TABLE workspace(id TEXT PRIMARY KEY, canonical_path BLOB NOT NULL UNIQUE);
                CREATE TABLE run(id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspace(id),
                    cmd TEXT NOT NULL, target_raw TEXT NOT NULL, started_at_ms INTEGER NOT NULL,
                    finished_at_ms INTEGER, bazel_elapsed_ms INTEGER, outcome TEXT NOT NULL,
                    exit_code INTEGER NOT NULL, log_truncated INTEGER NOT NULL);
                CREATE INDEX run_workspace_started ON run(workspace_id, started_at_ms DESC);
                PRAGMA user_version=1;")?;
            tx.commit()?;
        }
        validate_sidecars(&root)?;
        // Validate schema at startup even when there are no records.
        conn.prepare("SELECT r.id,w.canonical_path,r.target_raw FROM run r JOIN workspace w ON w.id=r.workspace_id LIMIT 0").map_err(|_| fail(ErrorKind::Corrupt))?;
        Ok(Self { conn, root })
    }
    fn workspace(&self, ws: &Path) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM workspace WHERE canonical_path=?1",
                [ws.as_os_str().as_bytes()],
                |r| r.get(0),
            )
            .optional()?)
    }
    fn artifacts(&self, workspace_id: &str, id: &str, create: bool) -> Result<Dir, StoreError> {
        let ws_id = uuid(workspace_id)?;
        let id = uuid(id)?;
        let runs = self
            .root
            .child("workspaces", create)?
            .child(&ws_id, create)?
            .child("runs", create)?;
        Ok(if create {
            runs.create_child(&id)?
        } else {
            runs.child(&id, false)?
        })
    }
}
fn validate_sidecars(root: &Dir) -> Result<(), StoreError> {
    for name in ["zeal.db-wal", "zeal.db-shm", "zeal.db-journal"] {
        if root.exists(name)? {
            root.file(name, false)?;
        }
    }
    Ok(())
}
pub fn ensure_ready(data: &Path) -> Result<(), StoreError> {
    Store::open(data).map(|_| ())
}

const SELECT: &str = "SELECT r.id,r.workspace_id,r.cmd,r.target_raw,r.started_at_ms,r.finished_at_ms,r.bazel_elapsed_ms,r.outcome,r.exit_code,r.log_truncated FROM run r JOIN workspace w ON w.id=r.workspace_id WHERE w.canonical_path=?1";
fn unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    value
        .try_into()
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
fn optional_unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    let value: Option<i64> = row.get(index)?;
    value
        .map(|n| {
            n.try_into()
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, n))
        })
        .transpose()
}
fn row_record(row: &rusqlite::Row<'_>, ws: &Path) -> rusqlite::Result<RunRecord> {
    Ok(RunRecord {
        id: row.get(0)?,
        workspace: ws.to_string_lossy().into_owned(),
        workspace_id: row.get(1)?,
        cmd: row.get(2)?,
        target: row.get(3)?,
        started_at: unsigned(row, 4)?,
        finished_at: optional_unsigned(row, 5)?,
        bazel_elapsed_ms: optional_unsigned(row, 6)?,
        outcome: row.get(7)?,
        exit_code: Some(row.get(8)?),
        log_truncated: row.get(9)?,
    })
}
pub fn list(data: &Path, ws: &Path) -> Result<Vec<RunRecord>, StoreError> {
    let ws = canonical(ws)?;
    let store = Store::open(data)?;
    let mut statement = store.conn.prepare(&format!(
        "{SELECT} ORDER BY r.started_at_ms DESC,r.id LIMIT {}",
        MAX_LIST_ROWS + 1
    ))?;
    let rows = statement.query_map([ws.as_os_str().as_bytes()], |row| row_record(row, &ws))?;
    let mut records = Vec::new();
    let mut text_bytes = 0;
    for row in rows {
        let rec = row?;
        text_bytes += rec.target.len() + rec.workspace.len() + rec.cmd.len() + 256;
        if records.len() == MAX_LIST_ROWS || text_bytes > MAX_LOG_BYTES {
            return Err(fail(ErrorKind::TooLarge));
        }
        records.push(rec);
    }
    Ok(records)
}
pub fn record(data: &Path, ws: &Path, id: &str) -> Result<RunRecord, StoreError> {
    let ws = canonical(ws)?;
    let id = uuid(id)?;
    let store = Store::open(data)?;
    Ok(store.conn.query_row(
        &format!("{SELECT} AND r.id=?2"),
        params![ws.as_os_str().as_bytes(), id],
        |row| row_record(row, &ws),
    )?)
}

/// Receiver-owned temporary file; a dropped, incomplete stream leaves no record.
pub struct PreparedLog {
    parent: Dir,
    dir: Dir,
    name: String,
    file: File,
    bytes: usize,
    truncated: bool,
    cleanup_attempted: bool,
    cleanup_failed: Option<Arc<AtomicBool>>,
}
impl PreparedLog {
    pub fn new(data: &Path) -> Result<Self, StoreError> {
        let root = Dir::root(data)?;
        let parent = root.child("tmp", true)?;
        let name = uuid::Uuid::new_v4().to_string();
        let dir = parent.create_child(&name)?;
        let file = dir.file("log.txt", true)?;
        Ok(Self {
            parent,
            dir,
            name,
            file,
            bytes: 0,
            truncated: false,
            cleanup_attempted: false,
            cleanup_failed: None,
        })
    }
    pub fn append(&mut self, text: &str) -> Result<(), StoreError> {
        if self.truncated {
            return Ok(());
        }
        let mut length = text.len().min(MAX_LOG_BYTES - self.bytes);
        while !text.is_char_boundary(length) {
            length -= 1;
        }
        self.file.write_all(&text.as_bytes()[..length])?;
        self.bytes += length;
        self.truncated |= length != text.len();
        Ok(())
    }
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}
impl PreparedLog {
    pub fn report_cleanup_to(&mut self, failed: Arc<AtomicBool>) {
        self.cleanup_failed = Some(failed);
    }
    fn cleanup(&mut self) -> Result<(), StoreError> {
        if self.cleanup_attempted {
            return Ok(());
        }
        self.cleanup_attempted = true;
        let result = (|| -> Result<(), StoreError> {
            if self.dir.exists("log.txt")? {
                self.dir.remove_file("log.txt")?;
            }
            Ok(self.parent.remove_dir(&self.name)?)
        })();
        if result.is_err()
            && let Some(failed) = &self.cleanup_failed
        {
            failed.store(true, Ordering::Release);
        }
        result
    }
    pub fn discard(mut self) -> Result<(), StoreError> {
        self.cleanup()
    }
}
impl Drop for PreparedLog {
    fn drop(&mut self) {
        if self.cleanup().is_err() {
            log::warn!("history temporary cleanup failed");
        }
    }
}
pub fn commit(
    data: &Path,
    ws: &Path,
    mut rec: RunRecord,
    mut log: PreparedLog,
) -> Result<RunRecord, StoreError> {
    let ws = canonical(ws)?;
    rec.id = uuid(&rec.id)?;
    if rec.started_at > MAX_TIME
        || !matches!(rec.cmd.as_str(), "build" | "test")
        || rec.exit_code.is_none()
    {
        return Err(fail(ErrorKind::Invalid));
    }
    rec.finished_at = rec.finished_at.filter(|time| *time <= MAX_TIME);
    rec.bazel_elapsed_ms = rec
        .finished_at
        .and_then(|end| end.checked_sub(rec.started_at));
    rec.outcome = match rec.exit_code {
        Some(0) => "ok",
        Some(8) => "interrupted",
        _ => "failed",
    }
    .into();
    rec.log_truncated = log.truncated();
    rec.workspace = ws.to_string_lossy().into_owned();
    let mut store = Store::open(data)?;
    rec.workspace_id = store
        .workspace(&ws)?
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    // A preexisting ID, including another workspace's ID, is never overwritten.
    if store
        .conn
        .query_row("SELECT 1 FROM run WHERE id=?1", [&rec.id], |_| Ok(()))
        .optional()?
        .is_some()
    {
        return Err(fail(ErrorKind::Conflict));
    }
    log.file.flush()?;
    log.file.sync_all()?;
    let run = store.artifacts(&rec.workspace_id, &rec.id, true)?;
    if let Err(error) = log.dir.rename_to("log.txt", &run, "log.txt") {
        // Keep a crash orphan if placement durability is uncertain.
        return Err(error.into());
    }
    let insert = (|| -> Result<(), StoreError> {
        let tx = store.conn.transaction()?;
        tx.execute("INSERT INTO workspace(id,canonical_path) VALUES(?1,?2) ON CONFLICT(canonical_path) DO NOTHING", params![rec.workspace_id,ws.as_os_str().as_bytes()])?;
        tx.execute("INSERT INTO run(id,workspace_id,cmd,target_raw,started_at_ms,finished_at_ms,bazel_elapsed_ms,outcome,exit_code,log_truncated) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![rec.id,rec.workspace_id,rec.cmd,rec.target,rec.started_at as i64,rec.finished_at.map(|n| n as i64),rec.bazel_elapsed_ms.map(|n| n as i64),rec.outcome,rec.exit_code,rec.log_truncated])?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(error) = insert {
        // A failed COMMIT can have an uncertain durability outcome. Confirm the
        // row before removing artifacts; an unreadable DB leaves an orphan.
        let saved: Option<String> = store
            .conn
            .query_row("SELECT workspace_id FROM run WHERE id=?1", [&rec.id], |r| {
                r.get(0)
            })
            .optional()?;
        if saved.as_deref() == Some(rec.workspace_id.as_str()) {
            return Ok(rec);
        }
        if saved.is_some() {
            return Err(error);
        }
        // Failure confirmed: delete only files this call just placed.
        if run.remove_file("log.txt").is_err() {
            return Err(fail(ErrorKind::Io));
        }
        let runs = store
            .root
            .child("workspaces", false)?
            .child(&rec.workspace_id, false)?
            .child("runs", false)?;
        runs.remove_dir(&rec.id)?;
        return Err(error);
    }
    // The summary is committed. A cleanup failure is reported through the
    // session flag, but cannot undo this successful invocation.
    if log.discard().is_err() {
        log::warn!("history temporary cleanup failed");
    }
    Ok(rec)
}

pub struct LogRead {
    pub text: String,
    pub truncated: bool,
}
pub fn read_log(data: &Path, ws: &Path, id: &str) -> Result<LogRead, StoreError> {
    let rec = record(data, ws, id)?;
    let store = Store::open(data)?;
    let file = store
        .artifacts(&rec.workspace_id, &rec.id, false)?
        .file("log.txt", false)?;
    let mut bytes = Vec::new();
    file.take((MAX_LOG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > MAX_LOG_BYTES;
    bytes.truncate(MAX_LOG_BYTES);
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    // UTF-8 replacement of invalid bytes can expand a hostile existing log.
    let mut end = MAX_LOG_BYTES.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = truncated || end < text.len();
    text.truncate(end);
    Ok(LogRead { text, truncated })
}
pub fn open_profile(data: &Path, ws: &Path, id: &str) -> Result<File, StoreError> {
    let rec = record(data, ws, id)?;
    let store = Store::open(data)?;
    Ok(store
        .artifacts(&rec.workspace_id, &rec.id, false)?
        .file("profile.gz", false)?)
}

/// Runner-owned directory. Cleanup is explicit because an unknown live process
/// may still be writing; Drop must never delete its destination.
pub struct ProfileDir {
    parent: Dir,
    dir: Dir,
    name: String,
}
impl ProfileDir {
    pub fn new(data: &Path) -> Result<Self, StoreError> {
        let root = Dir::root(data)?;
        let parent = root.child("tmp", true)?;
        let name = uuid::Uuid::new_v4().to_string();
        let dir = parent.create_child(&name)?;
        // Bazel truncates this existing file. Preserve 0600 without changing its umask.
        dir.file("profile.gz", true)?;
        Ok(Self { parent, dir, name })
    }
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
    pub fn open(&self) -> Result<File, StoreError> {
        Ok(self.dir.file("profile.gz", false)?)
    }
    pub fn cleanup(self) -> Result<(), StoreError> {
        if self.dir.exists("profile.gz")? {
            self.dir.remove_file("profile.gz")?;
        }
        Ok(self.parent.remove_dir(&self.name)?)
    }
}
pub fn attach_profile(
    data: &Path,
    ws: &Path,
    id: &str,
    source: &ProfileDir,
) -> Result<(), StoreError> {
    let rec = record(data, ws, id)?;
    let store = Store::open(data)?;
    let mut input = source.open()?.take(MAX_PROFILE_BYTES + 1);
    let run = store.artifacts(&rec.workspace_id, &rec.id, false)?;
    let mut output = run.file("profile.gz", true)?;
    let result = (|| -> Result<(), StoreError> {
        let bytes = io::copy(&mut input, &mut output)?;
        if bytes > MAX_PROFILE_BYTES {
            return Err(fail(ErrorKind::TooLarge));
        }
        output.flush()?;
        output.sync_all()?;
        Ok(run.sync()?)
    })();
    if result.is_err() {
        run.remove_file("profile.gz")?;
    }
    result
}

/// The workspace preference is a bounded private app setting, not a history row.
pub fn read_setting(data: &Path) -> Result<Vec<u8>, StoreError> {
    let root = Dir::root(data)?;
    let file = root.file("workspace.json", false)?;
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err(fail(ErrorKind::TooLarge));
    }
    Ok(bytes)
}
pub fn save_setting(data: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if bytes.len() > 65_536 {
        return Err(fail(ErrorKind::TooLarge));
    }
    let root = Dir::root(data)?;
    let name = format!("setting-{}", uuid::Uuid::new_v4());
    let mut file = root.file(&name, true)?;
    let result = (|| -> Result<(), StoreError> {
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(root.replace_setting(&name, "workspace.json")?)
    })();
    if result.is_err() {
        let _ = root.remove_file(&name);
    }
    result
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
            let root = std::env::temp_dir().join(format!("zeal-history-{}", uuid::Uuid::new_v4()));
            let data = root.join("data");
            let ws = root.join("workspace");
            fs::create_dir_all(&ws).unwrap();
            Self { root, data, ws }
        }
        fn record(&self) -> RunRecord {
            RunRecord {
                id: uuid::Uuid::new_v4().to_string(),
                workspace: String::new(),
                workspace_id: String::new(),
                cmd: "build".into(),
                target: "//pkg:all".into(),
                started_at: 100,
                finished_at: Some(125),
                exit_code: Some(0),
                bazel_elapsed_ms: None,
                outcome: String::new(),
                log_truncated: false,
            }
        }
        fn save(&self) -> RunRecord {
            let mut log = PreparedLog::new(&self.data).unwrap();
            log.append("hello\n").unwrap();
            commit(&self.data, &self.ws, self.record(), log).unwrap()
        }
        fn path(&self, rec: &RunRecord) -> PathBuf {
            self.data
                .join("workspaces")
                .join(&rec.workspace_id)
                .join("runs")
                .join(&rec.id)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn completed_summary_and_log_round_trip_with_private_modes() {
        let fixture = Fixture::new();
        let rec = fixture.save();
        assert_eq!(rec.bazel_elapsed_ms, Some(25));
        assert_eq!(rec.outcome, "ok");
        assert_eq!(list(&fixture.data, &fixture.ws).unwrap(), vec![rec.clone()]);
        assert_eq!(
            read_log(&fixture.data, &fixture.ws, &rec.id).unwrap().text,
            "hello\n"
        );
        assert_eq!(
            fs::metadata(&fixture.data).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(fixture.data.join("zeal.db"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(fixture.path(&rec).join("log.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn every_connection_enables_foreign_keys_wal_full_and_version() {
        let fixture = Fixture::new();
        let store = Store::open(&fixture.data).unwrap();
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
                .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "wal"
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
                .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn dropped_incomplete_stream_has_no_summary_or_log() {
        let fixture = Fixture::new();
        let mut log = PreparedLog::new(&fixture.data).unwrap();
        let temporary = log.dir.path().to_owned();
        log.append("partial").unwrap();
        drop(log);
        assert!(!temporary.exists());
        assert!(list(&fixture.data, &fixture.ws).unwrap().is_empty());
    }
    #[test]
    fn different_workspace_cannot_read_or_attach_existing_id() {
        let fixture = Fixture::new();
        let rec = fixture.save();
        let other = fixture.root.join("other");
        fs::create_dir(&other).unwrap();
        assert_eq!(
            record(&fixture.data, &other, &rec.id).unwrap_err().kind,
            ErrorKind::Missing
        );
        assert!(read_log(&fixture.data, &other, &rec.id).is_err());
        assert!(open_profile(&fixture.data, &other, &rec.id).is_err());
        let profile = ProfileDir::new(&fixture.data).unwrap();
        assert!(attach_profile(&fixture.data, &other, &rec.id, &profile).is_err());
        profile.cleanup().unwrap();
        assert!(list(&fixture.data, &other).unwrap().is_empty());
    }
    #[test]
    fn canonical_workspace_alias_shares_identity_and_invalid_path_is_rejected() {
        let fixture = Fixture::new();
        let rec = fixture.save();
        let alias = fixture.root.join("alias");
        symlink(&fixture.ws, &alias).unwrap();
        assert_eq!(record(&fixture.data, &alias, &rec.id).unwrap(), rec);
        assert!(list(&fixture.data, &fixture.root.join("missing")).is_err());
    }
    #[test]
    fn id_conflict_and_orphan_are_never_overwritten() {
        let fixture = Fixture::new();
        let rec = fixture.save();
        let log = PreparedLog::new(&fixture.data).unwrap();
        assert_eq!(
            commit(&fixture.data, &fixture.ws, rec.clone(), log)
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
        fs::remove_file(fixture.path(&rec).join("log.txt")).unwrap();
        let log = PreparedLog::new(&fixture.data).unwrap();
        assert_eq!(
            commit(&fixture.data, &fixture.ws, rec.clone(), log)
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
        assert!(!fixture.path(&rec).join("log.txt").exists());
        let fresh = fixture.record();
        fs::create_dir(fixture.path(&RunRecord {
            workspace_id: rec.workspace_id.clone(),
            ..fresh.clone()
        }))
        .unwrap();
        let log = PreparedLog::new(&fixture.data).unwrap();
        assert_eq!(
            commit(&fixture.data, &fixture.ws, fresh, log)
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
        assert_eq!(list(&fixture.data, &fixture.ws).unwrap(), vec![rec]);
    }
    #[test]
    fn details_deleted_after_commit_do_not_change_result_or_count() {
        let fixture = Fixture::new();
        let rec = fixture.save();
        fs::remove_dir_all(fixture.path(&rec)).unwrap();
        assert!(read_log(&fixture.data, &fixture.ws, &rec.id).is_err());
        assert_eq!(list(&fixture.data, &fixture.ws).unwrap(), vec![rec]);
    }
    #[test]
    fn truncation_is_a_utf8_prefix_and_sticky_across_later_progress() {
        let fixture = Fixture::new();
        let mut log = PreparedLog::new(&fixture.data).unwrap();
        log.append(&"a".repeat(MAX_LOG_BYTES - 1)).unwrap();
        log.append("あ").unwrap();
        log.append("z").unwrap();
        assert!(log.truncated());
        let rec = commit(&fixture.data, &fixture.ws, fixture.record(), log).unwrap();
        assert!(rec.log_truncated);
        let read = read_log(&fixture.data, &fixture.ws, &rec.id).unwrap();
        assert_eq!(read.text.len(), MAX_LOG_BYTES - 1);
        assert!(!read.truncated);
        assert!(!read.text.ends_with('z'));
    }
    #[test]
    fn read_time_truncation_is_distinct_from_save_time_truncation() {
        let fixture = Fixture::new();
        let rec = fixture.save();
        fs::write(
            fixture.path(&rec).join("log.txt"),
            vec![b'a'; MAX_LOG_BYTES + 10],
        )
        .unwrap();
        let read = read_log(&fixture.data, &fixture.ws, &rec.id).unwrap();
        assert_eq!(read.text.len(), MAX_LOG_BYTES);
        assert!(read.truncated);
        assert!(
            !record(&fixture.data, &fixture.ws, &rec.id)
                .unwrap()
                .log_truncated
        );
    }
    #[test]
    fn time_and_outcome_come_only_from_summary_values() {
        let fixture = Fixture::new();
        let mut input = fixture.record();
        input.finished_at = Some(99);
        input.exit_code = Some(8);
        let rec = commit(
            &fixture.data,
            &fixture.ws,
            input,
            PreparedLog::new(&fixture.data).unwrap(),
        )
        .unwrap();
        assert_eq!(rec.bazel_elapsed_ms, None);
        assert_eq!(rec.finished_at, Some(99));
        assert_eq!(rec.outcome, "interrupted");
        let mut input = fixture.record();
        input.finished_at = Some(MAX_TIME + 1);
        input.exit_code = Some(7);
        let rec = commit(
            &fixture.data,
            &fixture.ws,
            input,
            PreparedLog::new(&fixture.data).unwrap(),
        )
        .unwrap();
        assert_eq!(rec.finished_at, None);
        assert_eq!(rec.outcome, "failed");
    }
    #[test]
    fn invalid_ids_and_started_times_create_no_record() {
        let fixture = Fixture::new();
        let mut input = fixture.record();
        input.id = "../../escape".into();
        assert_eq!(
            commit(
                &fixture.data,
                &fixture.ws,
                input,
                PreparedLog::new(&fixture.data).unwrap()
            )
            .unwrap_err()
            .kind,
            ErrorKind::Invalid
        );
        let mut input = fixture.record();
        input.started_at = MAX_TIME + 1;
        assert_eq!(
            commit(
                &fixture.data,
                &fixture.ws,
                input,
                PreparedLog::new(&fixture.data).unwrap()
            )
            .unwrap_err()
            .kind,
            ErrorKind::Invalid
        );
        assert!(list(&fixture.data, &fixture.ws).unwrap().is_empty());
    }
    #[test]
    fn sidecar_without_database_refuses_automatic_initialization() {
        let fixture = Fixture::new();
        let root = Dir::root(&fixture.data).unwrap();
        root.file("zeal.db-wal", true).unwrap();
        assert_eq!(
            ensure_ready(&fixture.data).unwrap_err().kind,
            ErrorKind::Corrupt
        );
        assert!(!fixture.data.join("zeal.db").exists());
    }
    #[test]
    fn newer_and_corrupt_database_are_classified() {
        let fixture = Fixture::new();
        ensure_ready(&fixture.data).unwrap();
        let conn = Connection::open(fixture.data.join("zeal.db")).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        drop(conn);
        assert_eq!(
            ensure_ready(&fixture.data).unwrap_err().kind,
            ErrorKind::NewerSchema
        );
        fs::write(fixture.data.join("zeal.db"), b"not a SQLite file").unwrap();
        assert_eq!(
            ensure_ready(&fixture.data).unwrap_err().kind,
            ErrorKind::Corrupt
        );
    }
    #[test]
    fn symlink_fifo_and_hard_link_artifacts_are_rejected() {
        let fixture = Fixture::new();
        let rec = fixture.save();
        let path = fixture.path(&rec).join("log.txt");
        fs::remove_file(&path).unwrap();
        let outside = fixture.root.join("outside");
        fs::write(&outside, b"outside").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&outside, &path).unwrap();
        assert_eq!(
            read_log(&fixture.data, &fixture.ws, &rec.id)
                .err()
                .unwrap()
                .kind,
            ErrorKind::Permission
        );
        fs::remove_file(&path).unwrap();
        fs::hard_link(&outside, &path).unwrap();
        assert_eq!(
            read_log(&fixture.data, &fixture.ws, &rec.id)
                .err()
                .unwrap()
                .kind,
            ErrorKind::Permission
        );
        fs::remove_file(&path).unwrap();
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert_eq!(
            read_log(&fixture.data, &fixture.ws, &rec.id)
                .err()
                .unwrap()
                .kind,
            ErrorKind::Permission
        );
    }
    #[test]
    fn database_and_sidecar_links_are_rejected_before_sqlite_open() {
        let fixture = Fixture::new();
        ensure_ready(&fixture.data).unwrap();
        let outside = fixture.root.join("outside");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, fixture.data.join("zeal.db-wal")).unwrap();
        assert_eq!(
            ensure_ready(&fixture.data).unwrap_err().kind,
            ErrorKind::Permission
        );
        fs::remove_file(fixture.data.join("zeal.db-wal")).unwrap();
        fs::remove_file(fixture.data.join("zeal.db")).unwrap();
        symlink(&outside, fixture.data.join("zeal.db")).unwrap();
        assert_eq!(
            ensure_ready(&fixture.data).unwrap_err().kind,
            ErrorKind::Permission
        );
        assert_eq!(fs::read(outside).unwrap(), b"outside");
    }
    #[test]
    fn profile_attachment_is_exclusive_and_keeps_summary_immutable() {
        let fixture = Fixture::new();
        let rec = fixture.save();
        let profile = ProfileDir::new(&fixture.data).unwrap();
        fs::write(profile.path().join("profile.gz"), b"fixture").unwrap();
        attach_profile(&fixture.data, &fixture.ws, &rec.id, &profile).unwrap();
        assert_eq!(
            attach_profile(&fixture.data, &fixture.ws, &rec.id, &profile)
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
        profile.cleanup().unwrap();
        let mut bytes = Vec::new();
        open_profile(&fixture.data, &fixture.ws, &rec.id)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"fixture");
        assert_eq!(record(&fixture.data, &fixture.ws, &rec.id).unwrap(), rec);
    }
    #[test]
    fn private_setting_replaces_atomically_and_reads_bounded_bytes() {
        let fixture = Fixture::new();
        save_setting(&fixture.data, b"first").unwrap();
        save_setting(&fixture.data, b"second").unwrap();
        assert_eq!(read_setting(&fixture.data).unwrap(), b"second");
        assert_eq!(
            fs::metadata(fixture.data.join("workspace.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn ancestor_sync_failure_prevents_summary_commit() {
        let fixture = Fixture::new();
        ensure_ready(&fixture.data).unwrap();
        let log = PreparedLog::new(&fixture.data).unwrap();
        // Fail after workspaces is created, before the workspace directory is
        // exposed to the writer. This verifies propagation, not crash recovery.
        securefs::fail_directory_sync_under(
            fixture.data.canonicalize().unwrap().join("workspaces"),
        );
        assert_eq!(
            commit(&fixture.data, &fixture.ws, fixture.record(), log)
                .unwrap_err()
                .kind,
            ErrorKind::Io
        );
        assert!(list(&fixture.data, &fixture.ws).unwrap().is_empty());
    }
    #[test]
    fn cleanup_failure_after_commit_keeps_success_and_reports_once() {
        let fixture = Fixture::new();
        let mut log = PreparedLog::new(&fixture.data).unwrap();
        let failed = Arc::new(AtomicBool::new(false));
        log.report_cleanup_to(failed.clone());
        let temporary = log.dir.path().to_owned();
        fs::write(temporary.join("unexpected"), b"fixture").unwrap();
        let rec = commit(&fixture.data, &fixture.ws, fixture.record(), log).unwrap();
        assert!(failed.load(Ordering::Acquire));
        assert_eq!(record(&fixture.data, &fixture.ws, &rec.id).unwrap(), rec);
        fs::remove_file(temporary.join("unexpected")).unwrap();
        assert!(temporary.exists(), "cleanup is never retried automatically");
    }
}
