use super::securefs::Dir;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
#[derive(Clone, Copy, Debug)]
pub struct StoreError {
    pub kind: ErrorKind,
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            ErrorKind::Corrupt => "history database is corrupt or has an unsupported schema",
            ErrorKind::Permission => "history storage permission or file safety error",
            ErrorKind::DiskFull => "history storage is full",
            ErrorKind::Capacity => "history capacity limit reached",
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
pub(super) fn fail(kind: ErrorKind) -> StoreError {
    StoreError { kind }
}

/// Observed Bazel facts. Missing fields have not been reported by BEP.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvocationRecord {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

pub(super) fn id(value: &str) -> Result<String, StoreError> {
    uuid::Uuid::parse_str(value)
        .map(|v| v.to_string())
        .map_err(|_| fail(ErrorKind::Invalid))
}
pub(super) fn workspace_path(ws: &Path) -> Result<PathBuf, StoreError> {
    if !ws.is_absolute() {
        return Err(fail(ErrorKind::Invalid));
    }
    Ok(ws.canonicalize().unwrap_or_else(|_| ws.to_path_buf()))
}

pub(super) struct Store {
    pub(super) conn: Connection,
    root: Dir,
    _access: std::sync::MutexGuard<'static, ()>,
}
impl Store {
    pub(super) fn open(data: &Path) -> Result<Self, StoreError> {
        static ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let access = ACCESS.lock().expect("store access lock poisoned");
        let root = Dir::root(data)?;
        let exists = root.exists("zeal.db")?;
        validate_sidecars(&root, exists)?;
        let db = root.file("zeal.db", !exists)?;
        if exists && db.metadata()?.len() == 0 {
            return Err(fail(ErrorKind::Corrupt));
        }
        let mut conn = Connection::open_with_flags(
            root.path().join("zeal.db"),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        conn.busy_timeout(Duration::from_secs(2))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 3 {
            return Err(fail(ErrorKind::NewerSchema));
        }
        // Refuse old databases before changing journal mode or their schema.
        if exists && version != 3 {
            return Err(fail(ErrorKind::Corrupt));
        }
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        if !exists {
            let tx = conn.transaction()?;
            tx.execute_batch("CREATE TABLE workspace(id TEXT NOT NULL PRIMARY KEY,canonical_path BLOB NOT NULL UNIQUE);
                CREATE TABLE invocation(id TEXT NOT NULL PRIMARY KEY,workspace_id TEXT REFERENCES workspace(id),
                    cmd TEXT,target_pattern TEXT,started_at_ms INTEGER,finished_at_ms INTEGER,exit_code INTEGER,
                    attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt>=0),
                    received_complete INTEGER NOT NULL DEFAULT 0 CHECK(received_complete IN (0,1)));
                CREATE INDEX invocation_workspace_started ON invocation(workspace_id,started_at_ms DESC,id);
                PRAGMA user_version=3;")?;
            tx.commit()?;
            db.sync_all()?;
            root.sync()?;
        }
        conn.prepare("SELECT id,workspace_id,cmd,target_pattern,started_at_ms,finished_at_ms,exit_code,attempt,received_complete FROM invocation LIMIT 0").map_err(|_| fail(ErrorKind::Corrupt))?;
        conn.prepare("SELECT id,canonical_path FROM workspace LIMIT 0")
            .map_err(|_| fail(ErrorKind::Corrupt))?;
        Ok(Self {
            conn,
            root,
            _access: access,
        })
    }
    pub(super) fn invocation_dir(
        &self,
        invocation_id: &str,
        create: bool,
    ) -> Result<Dir, StoreError> {
        Ok(self
            .root
            .child("invocations", create)?
            .child(&id(invocation_id)?, create)?)
    }
}

/// Check every existing sidecar before opening or creating the database.
fn validate_sidecars(root: &Dir, exists: bool) -> Result<(), StoreError> {
    for name in ["zeal.db-wal", "zeal.db-shm", "zeal.db-journal"] {
        if !root.exists(name)? {
            continue;
        }
        if !exists {
            return Err(fail(ErrorKind::Corrupt));
        }
        root.file(name, false)?;
    }
    Ok(())
}

pub fn reserve_invocation(data: &Path, invocation_id: &str) -> Result<(), StoreError> {
    let invocation_id = id(invocation_id)?;
    let mut store = Store::open(data)?;
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("INSERT INTO invocation(id) VALUES(?1)", [&invocation_id])?;
    let root = Dir::root(data)?.child("invocations", true)?;
    root.create_child(&invocation_id)?;
    tx.commit()?;
    Ok(())
}

const SELECT: &str = "SELECT r.id,r.cmd,r.target_pattern,r.started_at_ms,r.finished_at_ms,r.exit_code FROM invocation r JOIN workspace w ON w.id=r.workspace_id WHERE w.canonical_path=?1";
/// [`list`] returns at most this many records, newest first.
pub const LIST_LIMIT: usize = 100;
fn row_record(row: &rusqlite::Row<'_>, ws: &Path) -> rusqlite::Result<InvocationRecord> {
    fn time(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<Option<u64>> {
        row.get::<_, Option<i64>>(column)?
            .map(|v| {
                v.try_into()
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, v))
            })
            .transpose()
    }
    Ok(InvocationRecord {
        id: row.get(0)?,
        workspace: Some(ws.to_string_lossy().into_owned()),
        cmd: row.get(1)?,
        pattern: row.get(2)?,
        started_at: time(row, 3)?,
        finished_at: time(row, 4)?,
        exit_code: row.get(5)?,
    })
}
pub fn read_record(
    data: &Path,
    ws: &Path,
    invocation_id: &str,
) -> Result<InvocationRecord, StoreError> {
    let ws = workspace_path(ws)?;
    let store = Store::open(data)?;
    Ok(store.conn.query_row(
        &format!("{SELECT} AND r.id=?2"),
        params![ws.as_os_str().as_bytes(), id(invocation_id)?],
        |r| row_record(r, &ws),
    )?)
}
pub fn list(data: &Path, ws: &Path) -> Result<Vec<InvocationRecord>, StoreError> {
    let ws = workspace_path(ws)?;
    let store = Store::open(data)?;
    Ok(store
        .conn
        .prepare(&format!(
            "{SELECT} ORDER BY r.started_at_ms DESC,r.id LIMIT {LIST_LIMIT}"
        ))?
        .query_map([ws.as_os_str().as_bytes()], |r| row_record(r, &ws))?
        .collect::<Result<_, _>>()?)
}
pub fn artifact(data: &Path, ws: &Path, invocation_id: &str, name: &str) -> io::Result<File> {
    attempt_artifact(data, ws, invocation_id, name)
        .map(|(file, _)| file)
        .map_err(Into::into)
}

pub(crate) fn event_artifact(
    data: &Path,
    ws: &Path,
    invocation_id: &str,
) -> Result<(File, bool), StoreError> {
    attempt_artifact(data, ws, invocation_id, "bep.binpb.gz")
}

/// Snapshot the current receipt and its checked FD before doing any parsing.
fn attempt_artifact(
    data: &Path,
    ws: &Path,
    invocation_id: &str,
    name: &str,
) -> Result<(File, bool), StoreError> {
    if !matches!(name, "bep.binpb.gz" | "profile.json.gz") {
        return Err(fail(ErrorKind::Invalid));
    }
    read_record(data, ws, invocation_id)?;
    let store = Store::open(data)?;
    let (attempt, complete): (i64, bool) = store.conn.query_row(
        "SELECT attempt,received_complete FROM invocation WHERE id=?1",
        [id(invocation_id)?],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if attempt == 0 {
        return Err(fail(ErrorKind::Missing));
    }
    let file = store
        .invocation_dir(invocation_id, false)?
        .child("attempt", false)?
        .child(&attempt.to_string(), false)?
        .artifact(name)?;
    Ok((file, complete))
}
pub fn abort_reservation(data: &Path, invocation_id: &str) -> Result<(), StoreError> {
    let invocation_id = id(invocation_id)?;
    let mut store = Store::open(data)?;
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let attempt: i64 = tx.query_row(
        "SELECT attempt FROM invocation WHERE id=?1",
        [&invocation_id],
        |r| r.get(0),
    )?;
    if attempt != 0 {
        return Err(fail(ErrorKind::Conflict));
    }
    let invocations = Dir::root(data)?.child("invocations", false)?;
    invocations.child(&invocation_id, false)?;
    invocations.remove_dir(&invocation_id)?;
    tx.execute(
        "DELETE FROM invocation WHERE id=?1 AND attempt=0",
        [invocation_id],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::events::test_support::Fixture;
    use std::fs;
    use std::io::Read;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn schema_is_two_tables_and_old_schema_is_refused_without_modification() {
        let f = Fixture::new();
        {
            let store = Store::open(&f.data).unwrap();
            let tables: Vec<String> = store
                .conn
                .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(tables, ["invocation", "workspace"]);
            store
                .conn
                .pragma_update(None, "journal_mode", "DELETE")
                .unwrap();
            store.conn.pragma_update(None, "user_version", 2).unwrap();
        }
        let path = f.data.join("zeal.db");
        let bytes = fs::read(&path).unwrap();
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn corrupt_newer_and_orphan_sidecars_are_not_reset() {
        let f = Fixture::new();
        let root = Dir::root(&f.data).unwrap();
        root.file("zeal.db-wal", true).unwrap();
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
        assert!(!f.data.join("zeal.db").exists());
        root.remove_file("zeal.db-wal").unwrap();
        fs::write(f.data.join("zeal.db"), b"invalid").unwrap();
        fs::set_permissions(f.data.join("zeal.db"), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
        fs::remove_file(f.data.join("zeal.db")).unwrap();
        let store = Store::open(&f.data).unwrap();
        store.conn.pragma_update(None, "user_version", 4).unwrap();
        drop(store);
        assert_eq!(
            Store::open(&f.data).err().unwrap().kind,
            ErrorKind::NewerSchema
        );
    }
    #[test]
    fn invocations_are_invisible_to_unobserved_workspaces() {
        let f = Fixture::new();
        f.complete();
        let invocation_id = &f.stream.invocation_id;
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        assert!(list(&f.data, &other).unwrap().is_empty());
        assert_eq!(
            read_record(&f.data, &other, invocation_id)
                .unwrap_err()
                .kind,
            ErrorKind::Missing
        );
        assert!(artifact(&f.data, &other, invocation_id, "bep.binpb.gz").is_err());
    }
    #[test]
    fn workspace_aliases_list_the_same_invocations() {
        let f = Fixture::new();
        f.complete();
        let alias = f.root.join("alias");
        symlink(&f.ws, &alias).unwrap();
        assert_eq!(
            list(&f.data, &alias).unwrap(),
            list(&f.data, &f.ws).unwrap()
        );
    }
    #[test]
    fn artifact_paths_and_symlinks_cannot_escape_the_invocation_directory() {
        let f = Fixture::new();
        f.complete();
        let invocation_id = &f.stream.invocation_id;
        assert!(artifact(&f.data, &f.ws, "../../escape", "bep.binpb.gz").is_err());
        assert!(artifact(&f.data, &f.ws, invocation_id, "../zeal.db").is_err());
        let path = f.dir().join("attempt/1/bep.binpb.gz");
        fs::remove_file(&path).unwrap();
        symlink(f.data.join("zeal.db"), &path).unwrap();
        assert_eq!(
            artifact(&f.data, &f.ws, invocation_id, "bep.binpb.gz")
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    #[test]
    fn profile_uses_the_current_attempt() {
        let f = Fixture::new();
        f.complete();
        fs::write(f.dir().join("attempt/1/profile.json.gz"), b"current").unwrap();

        let mut profile = String::new();
        artifact(&f.data, &f.ws, &f.stream.invocation_id, "profile.json.gz")
            .unwrap()
            .read_to_string(&mut profile)
            .unwrap();

        assert_eq!(profile, "current");
    }
    #[test]
    fn unsafe_attempt_profile_is_rejected() {
        let f = Fixture::new();
        f.complete();
        symlink(
            f.data.join("zeal.db"),
            f.dir().join("attempt/1/profile.json.gz"),
        )
        .unwrap();

        assert_eq!(
            artifact(&f.data, &f.ws, &f.stream.invocation_id, "profile.json.gz")
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    #[test]
    fn profile_from_an_obsolete_attempt_is_not_used() {
        use crate::store::events::Receipt;

        let f = Fixture::new();
        let mut first = Receipt::begin(&f.data, &f.stream).unwrap();
        first.append(&f.ordered(1, f.started())).unwrap();
        first.partial().unwrap();
        fs::write(f.dir().join("attempt/1/profile.json.gz"), b"obsolete").unwrap();
        let second = Receipt::begin(&f.data, &f.stream).unwrap();
        second.partial().unwrap();

        assert_eq!(
            artifact(&f.data, &f.ws, &f.stream.invocation_id, "profile.json.gz")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
    #[test]
    fn invocation_records_survive_when_invocation_details_disappear() {
        let f = Fixture::new();
        f.complete();
        fs::remove_dir_all(f.dir()).unwrap();
        assert_eq!(list(&f.data, &f.ws).unwrap().len(), 1);
    }
    #[test]
    fn list_returns_at_most_list_limit_newest_records() {
        let f = Fixture::new();
        f.complete();
        let store = Store::open(&f.data).unwrap();
        let ws_id: String = store
            .conn
            .query_row("SELECT id FROM workspace", [], |r| r.get(0))
            .unwrap();
        for i in 0..LIST_LIMIT {
            store
                .conn
                .execute(
                    "INSERT INTO invocation(id,workspace_id,started_at_ms) VALUES(?1,?2,?3)",
                    params![uuid::Uuid::new_v4().to_string(), ws_id, 2000 + i as i64],
                )
                .unwrap();
        }
        drop(store);
        let invocations = list(&f.data, &f.ws).unwrap();
        assert_eq!(invocations.len(), LIST_LIMIT);
        assert_eq!(
            invocations[0].started_at,
            Some(2000 + LIST_LIMIT as u64 - 1)
        );
        assert!(
            invocations
                .iter()
                .all(|invocation| invocation.id != f.stream.invocation_id)
        );
    }
    #[test]
    fn a_reservation_is_usable_before_any_workspace_observes_it() {
        let f = Fixture::new();
        let invocation_id = &f.stream.invocation_id;
        reserve_invocation(&f.data, invocation_id).unwrap();
        assert_eq!(
            reserve_invocation(&f.data, invocation_id).unwrap_err().kind,
            ErrorKind::Conflict
        );
        assert!(list(&f.data, &f.ws).unwrap().is_empty());
        assert!(!f.dir().join("terminal.log").exists());
    }
    #[test]
    fn aborting_a_reservation_preserves_foreign_files_and_frees_the_invocation() {
        let f = Fixture::new();
        let invocation_id = &f.stream.invocation_id;
        reserve_invocation(&f.data, invocation_id).unwrap();
        let dir = f.dir();
        fs::write(dir.join("unexpected"), b"keep").unwrap();
        assert!(abort_reservation(&f.data, invocation_id).is_err());
        assert!(dir.join("unexpected").exists());
        fs::remove_file(dir.join("unexpected")).unwrap();
        abort_reservation(&f.data, invocation_id).unwrap();
        assert!(!dir.exists());
        reserve_invocation(&f.data, invocation_id).unwrap();
    }
    #[test]
    fn a_completed_invocation_cannot_be_aborted() {
        let f = Fixture::new();
        f.complete();
        assert_eq!(
            abort_reservation(&f.data, &f.stream.invocation_id)
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
    }
    #[test]
    fn independent_readers_keep_sqlite_sidecars_safe() {
        let f = Fixture::new();
        f.complete();
        std::thread::scope(|scope| spawn_history_readers(scope, &f));
    }

    fn spawn_history_readers<'scope>(
        scope: &'scope std::thread::Scope<'scope, '_>,
        f: &'scope Fixture,
    ) {
        for _ in 0..4 {
            scope.spawn(move || read_history_repeatedly(f));
        }
    }

    #[test]
    fn linked_database_sidecars_and_missing_schema_tables_are_rejected() {
        let f = Fixture::new();
        drop(Store::open(&f.data).unwrap());
        symlink(f.data.join("zeal.db"), f.data.join("zeal.db-wal")).unwrap();
        assert_eq!(
            Store::open(&f.data).err().unwrap().kind,
            ErrorKind::Permission
        );
        fs::remove_file(f.data.join("zeal.db-wal")).unwrap();
        let store = Store::open(&f.data).unwrap();
        store.conn.execute_batch("DROP TABLE workspace").unwrap();
        drop(store);
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
    }

    fn read_history_repeatedly(f: &Fixture) {
        for _ in 0..10 {
            assert_eq!(list(&f.data, &f.ws).unwrap().len(), 1);
        }
    }
}
