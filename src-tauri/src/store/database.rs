use super::securefs::Dir;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
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
            ErrorKind::NewerSchema => "history database requires a newer Zelith version",
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
        let exists = root.exists("zelith.db")?;
        validate_sidecars(&root, exists)?;
        let db = root.file("zelith.db", !exists)?;
        if exists && db.metadata()?.len() == 0 {
            return Err(fail(ErrorKind::Corrupt));
        }
        let mut conn = Connection::open_with_flags(
            root.path().join("zelith.db"),
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
            tx.execute_batch("CREATE TABLE workspace(id TEXT NOT NULL PRIMARY KEY,canonical_path BLOB NOT NULL UNIQUE,repo_url TEXT);
                CREATE INDEX workspace_repo_url ON workspace(repo_url);
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
        conn.prepare("SELECT id,canonical_path,repo_url FROM workspace LIMIT 0")
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
    for name in ["zelith.db-wal", "zelith.db-shm", "zelith.db-journal"] {
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

const RECORD_COLS: &str =
    "r.id,r.cmd,r.target_pattern,r.started_at_ms,r.finished_at_ms,r.exit_code,w.canonical_path";
const WORKSPACE_SCOPE: &str = "FROM invocation r JOIN workspace w ON w.id=r.workspace_id JOIN workspace current ON current.canonical_path=?1 WHERE (w.id=current.id OR w.repo_url=current.repo_url)";
/// Number of recent records returned by [`list`], before any pinned records.
pub const LIST_LIMIT: usize = 100;
fn row_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationRecord> {
    fn time(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<Option<u64>> {
        row.get::<_, Option<i64>>(column)?
            .map(|v| {
                v.try_into()
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, v))
            })
            .transpose()
    }
    let path: Vec<u8> = row.get(6)?;
    let ws = PathBuf::from(std::ffi::OsString::from_vec(path));
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
        &format!("SELECT {RECORD_COLS} {WORKSPACE_SCOPE} AND r.id=?2"),
        params![ws.as_os_str().as_bytes(), id(invocation_id)?],
        row_record,
    )?)
}
/// List the newest records and any pinned IDs in the same repository scope.
pub fn list(
    data: &Path,
    ws: &Path,
    pinned_ids: &[String],
) -> Result<Vec<InvocationRecord>, StoreError> {
    let ws = workspace_path(ws)?;
    let store = Store::open(data)?;
    let pinned_ids = serde_json::to_string(pinned_ids).expect("serialize pinned invocation IDs");
    Ok(store
        .conn
        .prepare(&format!(
            "WITH recent AS (SELECT {RECORD_COLS} {WORKSPACE_SCOPE} ORDER BY r.started_at_ms DESC,r.id LIMIT {LIST_LIMIT})
             SELECT * FROM recent
             UNION SELECT {RECORD_COLS} {WORKSPACE_SCOPE} AND r.id IN (SELECT value FROM json_each(?2))
             ORDER BY started_at_ms DESC,id"
        ))?
        .query_map(params![ws.as_os_str().as_bytes(), pinned_ids], row_record)?
        .collect::<Result<_, _>>()?)
}

pub fn frequent_workspaces(data: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let store = Store::open(data)?;
    Ok(store
        .conn
        .prepare(
            "WITH recent AS (
                SELECT workspace_id,started_at_ms FROM invocation
                ORDER BY started_at_ms DESC,id LIMIT 100
            )
            SELECT w.canonical_path FROM recent r
            JOIN workspace w ON w.id=r.workspace_id
            GROUP BY w.id
            ORDER BY COUNT(*) DESC,MAX(r.started_at_ms) DESC,w.canonical_path
            LIMIT 5",
        )?
        .query_map([], |row| {
            let path: Vec<u8> = row.get(0)?;
            Ok(PathBuf::from(std::ffi::OsString::from_vec(path)))
        })?
        .collect::<Result<_, _>>()?)
}

pub fn artifact(data: &Path, ws: &Path, invocation_id: &str, name: &str) -> io::Result<File> {
    attempt_artifact(data, ws, invocation_id, name)
        .map(|(file, _)| file)
        .map_err(Into::into)
}

pub fn refresh_workspace(data: &Path, ws: &Path, repo_url: Option<&str>) -> Result<(), StoreError> {
    let ws = workspace_path(ws)?;
    let mut store = Store::open(data)?;
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    save_workspace(&tx, ws.as_os_str().as_bytes(), repo_url)?;
    tx.commit()?;
    Ok(())
}

pub(super) fn save_workspace(
    tx: &rusqlite::Transaction<'_>,
    path: &[u8],
    repo_url: Option<&str>,
) -> Result<String, StoreError> {
    tx.execute(
        "INSERT INTO workspace(id,canonical_path,repo_url) VALUES(?1,?2,?3) ON CONFLICT(canonical_path) DO UPDATE SET repo_url=excluded.repo_url",
        params![uuid::Uuid::new_v4().to_string(), path, repo_url],
    )?;
    Ok(tx.query_row(
        "SELECT id FROM workspace WHERE canonical_path=?1",
        [path],
        |r| r.get(0),
    )?)
}

pub(crate) fn event_artifact(
    data: &Path,
    ws: &Path,
    invocation_id: &str,
) -> Result<(File, bool), StoreError> {
    attempt_artifact(data, ws, invocation_id, "bep.binpb.zst")
}

/// Snapshot the current receipt and its checked FD before doing any parsing.
fn attempt_artifact(
    data: &Path,
    ws: &Path,
    invocation_id: &str,
    name: &str,
) -> Result<(File, bool), StoreError> {
    if !matches!(name, "bep.binpb.zst" | "profile.json.gz") {
        return Err(fail(ErrorKind::Invalid));
    }
    let ws = workspace_path(ws)?;
    let invocation_id = id(invocation_id)?;
    let store = Store::open(data)?;
    let (attempt, complete): (i64, bool) = store.conn.query_row(
        &format!("SELECT r.attempt,r.received_complete {WORKSPACE_SCOPE} AND r.id=?2"),
        params![ws.as_os_str().as_bytes(), &invocation_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if attempt == 0 || (name == "profile.json.gz" && !complete) {
        return Err(fail(ErrorKind::Missing));
    }
    let file = store
        .invocation_dir(&invocation_id, false)?
        .child("attempt", false)?
        .child(&attempt.to_string(), false)?
        .artifact(name)?;
    Ok((file, complete))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Core, ProcessDiagnostic};
    use crate::store::events::test_support::Fixture;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn frequent_workspaces_ranks_paths_within_the_latest_hundred_invocations() {
        let f = Fixture::new();
        let mut store = Store::open(&f.data).unwrap();
        let tx = store.conn.transaction().unwrap();
        let workspaces = [
            ("old", 200),
            ("a", 40),
            ("b", 25),
            ("c", 15),
            ("d", 10),
            ("e", 5),
            ("f", 5),
        ];
        for (name, _) in workspaces {
            let path = f.root.join(name);
            let repo = match name {
                "a" | "b" => Some("https://example.com/shared.git"),
                _ => None,
            };
            tx.execute(
                "INSERT INTO workspace(id,canonical_path,repo_url) VALUES(?1,?2,?3)",
                params![name, path.as_os_str().as_bytes(), repo],
            )
            .unwrap();
        }
        for (index, name) in workspaces
            .into_iter()
            .flat_map(|(name, count)| std::iter::repeat_n(name, count))
            .enumerate()
        {
            tx.execute(
                "INSERT INTO invocation(id,workspace_id,started_at_ms) VALUES(?1,?2,?3)",
                params![format!("{name}-{index}"), name, index as i64 + 1],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(store);
        let candidates = frequent_workspaces(&f.data).unwrap();
        assert_eq!(
            candidates,
            ["a", "b", "c", "d", "f"].map(|name| f.root.join(name))
        );
        assert!(candidates.iter().all(|path| !path.exists()));
    }

    #[test]
    fn frequent_workspaces_limits_before_discarding_records_without_a_workspace() {
        let f = Fixture::new();
        assert!(frequent_workspaces(&f.data).unwrap().is_empty());
        f.complete();
        let mut store = Store::open(&f.data).unwrap();
        let tx = store.conn.transaction().unwrap();
        for index in 0..100 {
            tx.execute(
                "INSERT INTO invocation(id,started_at_ms) VALUES(?1,?2)",
                params![format!("unknown-{index}"), 3000 + index],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(store);
        assert!(frequent_workspaces(&f.data).unwrap().is_empty());
    }

    #[test]
    fn frequent_workspaces_keeps_untimed_records_and_breaks_ties_by_path() {
        let f = Fixture::new();
        let store = Store::open(&f.data).unwrap();
        for name in ["b", "a"] {
            store
                .conn
                .execute(
                    "INSERT INTO workspace(id,canonical_path) VALUES(?1,?2)",
                    params![name, f.root.join(name).as_os_str().as_bytes()],
                )
                .unwrap();
            store
                .conn
                .execute(
                    "INSERT INTO invocation(id,workspace_id) VALUES(?1,?1)",
                    [name],
                )
                .unwrap();
        }
        drop(store);
        assert_eq!(
            frequent_workspaces(&f.data).unwrap(),
            vec![f.root.join("a"), f.root.join("b")]
        );
    }

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
        let path = f.data.join("zelith.db");
        let bytes = fs::read(&path).unwrap();
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn corrupt_newer_and_orphan_sidecars_are_not_reset() {
        let f = Fixture::new();
        let root = Dir::root(&f.data).unwrap();
        root.file("zelith.db-wal", true).unwrap();
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
        assert!(!f.data.join("zelith.db").exists());
        fs::remove_file(f.data.join("zelith.db-wal")).unwrap();
        fs::write(f.data.join("zelith.db"), b"invalid").unwrap();
        fs::set_permissions(f.data.join("zelith.db"), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
        fs::remove_file(f.data.join("zelith.db")).unwrap();
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
        assert!(list(&f.data, &other, &[]).unwrap().is_empty());
        assert_eq!(
            read_record(&f.data, &other, invocation_id)
                .unwrap_err()
                .kind,
            ErrorKind::Missing
        );
        assert!(artifact(&f.data, &other, invocation_id, "bep.binpb.zst").is_err());
    }
    #[test]
    fn workspace_aliases_list_the_same_invocations() {
        let f = Fixture::new();
        f.complete();
        let alias = f.root.join("alias");
        symlink(&f.ws, &alias).unwrap();
        assert_eq!(
            list(&f.data, &alias, &[]).unwrap(),
            list(&f.data, &f.ws, &[]).unwrap()
        );
    }
    #[test]
    fn artifact_paths_and_symlinks_cannot_escape_the_invocation_directory() {
        let f = Fixture::new();
        f.complete();
        let invocation_id = &f.stream.invocation_id;
        assert!(artifact(&f.data, &f.ws, "../../escape", "bep.binpb.zst").is_err());
        assert!(artifact(&f.data, &f.ws, invocation_id, "../zelith.db").is_err());
        let path = f.dir().join("attempt/1/bep.binpb.zst");
        fs::remove_file(&path).unwrap();
        symlink(f.data.join("zelith.db"), &path).unwrap();
        assert_eq!(
            artifact(&f.data, &f.ws, invocation_id, "bep.binpb.zst")
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    #[test]
    fn invocation_records_survive_when_invocation_details_disappear() {
        let f = Fixture::new();
        f.complete();
        fs::remove_dir_all(f.dir()).unwrap();
        assert_eq!(list(&f.data, &f.ws, &[]).unwrap().len(), 1);
    }
    fn workspace_id(store: &Store, ws: &Path) -> String {
        store
            .conn
            .query_row(
                "SELECT id FROM workspace WHERE canonical_path=?1",
                [ws.canonicalize().unwrap().as_os_str().as_bytes()],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// Fill the recent window so earlier records need a pin to resurface.
    fn insert_recent_window(store: &Store, ws_ids: &[String]) -> Vec<String> {
        let ids: Vec<_> = (0..LIST_LIMIT).map(|i| format!("recent-{i}")).collect();
        for (i, id) in ids.iter().enumerate() {
            store
                .conn
                .execute(
                    "INSERT INTO invocation(id,workspace_id,started_at_ms) VALUES(?1,?2,?3)",
                    params![id, &ws_ids[i % ws_ids.len()], 2000 + i as i64],
                )
                .unwrap();
        }
        ids
    }

    /// Seed the in-memory cache the way a finished worker leaves it.
    fn cache_diagnostic(core: &Core, ws: &Path, id: &str) {
        core.state
            .lock()
            .unwrap()
            .diagnostics
            .push_back(ProcessDiagnostic {
                id: id.to_string(),
                workspace: ws.to_string_lossy().into_owned(),
                cmd: "build".into(),
                pattern: "//:app".into(),
                started_at: 1,
                process_exit_code: Some(9),
                output: "tail".into(),
            });
    }

    #[test]
    fn list_includes_pinned_records_older_than_its_recent_limit() {
        let (f, sibling) = shared_repo_pair();
        let store = Store::open(&f.data).unwrap();
        let ids = [
            workspace_id(&store, &f.ws),
            workspace_id(&store, &sibling.ws),
        ];
        let recent_ids = insert_recent_window(&store, &ids);
        drop(store);
        let recent = list(&f.data, &f.ws, &[]).unwrap();
        assert!(recent.iter().map(|r| &r.id).eq(recent_ids.iter().rev()));

        let selected_id = &f.stream.invocation_id;
        let live_id = &sibling.stream.invocation_id;
        let pinned = [
            selected_id.clone(),
            live_id.clone(),
            recent_ids.last().unwrap().clone(),
            live_id.clone(),
        ];
        let invocations = list(&f.data, &f.ws, &pinned).unwrap();
        assert_eq!(&invocations[..LIST_LIMIT], recent.as_slice());
        let mut expected = [selected_id, live_id];
        expected.sort();
        assert!(invocations[LIST_LIMIT..].iter().map(|r| &r.id).eq(expected));
    }

    /// A diagnostic whose record lands after the fact is retired by the
    /// next history read: its id joins the store lookup only, so the aged
    /// record is fetched for the check but never exposed unselected, and
    /// later calls cannot resurrect the diagnostic.
    #[test]
    fn history_list_retires_a_diagnostic_once_its_record_is_persisted() {
        let (f, sibling) = shared_repo_pair();
        let ws = f.ws.canonicalize().unwrap();
        let store = Store::open(&f.data).unwrap();
        let ids = [workspace_id(&store, &ws), workspace_id(&store, &sibling.ws)];
        insert_recent_window(&store, &ids);
        drop(store);

        let core = Core::new();
        let recorded = f.stream.invocation_id.clone();
        cache_diagnostic(&core, &ws, &recorded);

        let history = crate::load_invocation_history(&core, &ws, None, Ok(f.data.clone()));
        let saved = history.saved.unwrap();
        assert_eq!(saved.len(), LIST_LIMIT);
        assert!(saved.iter().all(|record| record.id.starts_with("recent-")));
        assert!(history.diagnostics.is_empty());
        assert!(core.state.lock().unwrap().diagnostics.is_empty());

        // The selected pin still surfaces the same record.
        let history =
            crate::load_invocation_history(&core, &ws, Some(recorded.clone()), Ok(f.data.clone()));
        let saved = history.saved.unwrap();
        assert!(saved.iter().any(|record| record.id == recorded));
        assert!(history.diagnostics.is_empty());

        let history = crate::load_invocation_history(&core, &ws, None, Ok(f.data.clone()));
        assert!(history.diagnostics.is_empty());
    }

    /// Repo scope is not ownership: a same-id record owned by a sibling
    /// workspace cannot retire this workspace's diagnostic, and a store
    /// failure keeps the diagnostic while reporting the error itself.
    #[test]
    fn history_list_keeps_diagnostics_without_a_matching_owner() {
        let (f, sibling) = shared_repo_pair();
        let ws = f.ws.canonicalize().unwrap();
        let core = Core::new();
        let sibling_id = sibling.stream.invocation_id.clone();
        cache_diagnostic(&core, &ws, &sibling_id);

        let history = crate::load_invocation_history(&core, &ws, None, Ok(f.data.clone()));
        assert!(history.saved.is_ok());
        assert_eq!(history.diagnostics.len(), 1);
        assert_eq!(history.diagnostics[0].id, sibling_id);
        assert_eq!(core.state.lock().unwrap().diagnostics.len(), 1);

        let history = crate::load_invocation_history(
            &core,
            &ws,
            None,
            Err("specific path error".to_string()),
        );
        assert_eq!(
            history.saved.as_ref().unwrap_err().as_str(),
            "specific path error"
        );
        assert_eq!(history.diagnostics.len(), 1);
        assert_eq!(core.state.lock().unwrap().diagnostics.len(), 1);
    }

    fn shared_repo_pair() -> (Fixture, Fixture) {
        let f = Fixture::new();
        let mut sibling = Fixture::new();
        sibling.data = f.data.clone();
        crate::workspace::test_support::repo(&f.ws, "https://example.com/shared.git");
        crate::workspace::test_support::repo(&sibling.ws, "https://example.com/shared.git");
        f.complete();
        sibling.complete();
        (f, sibling)
    }

    #[test]
    fn same_repo_invocations_are_listed_across_workspaces() {
        let (f, sibling) = shared_repo_pair();
        let own_id = f.stream.invocation_id.clone();
        let sibling_id = sibling.stream.invocation_id.clone();
        let own_path = f.ws.canonicalize().unwrap().to_string_lossy().into_owned();
        let sibling_path = sibling
            .ws
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let from_first = list(&f.data, &f.ws, &[]).unwrap();
        let from_second = list(&f.data, &sibling.ws, &[]).unwrap();
        assert_eq!(from_first, from_second);
        assert_eq!(from_first.len(), 2);
        assert_eq!(
            from_first
                .iter()
                .find(|record| record.id == own_id)
                .unwrap()
                .workspace
                .as_deref(),
            Some(own_path.as_str())
        );
        assert_eq!(
            from_first
                .iter()
                .find(|record| record.id == sibling_id)
                .unwrap()
                .workspace
                .as_deref(),
            Some(sibling_path.as_str())
        );
    }

    #[test]
    fn same_repo_records_artifacts_and_details_cross_workspaces() {
        let (f, sibling) = shared_repo_pair();
        let sibling_id = sibling.stream.invocation_id.clone();
        let sibling_path = sibling
            .ws
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let record = read_record(&f.data, &f.ws, &sibling_id).unwrap();
        assert_eq!(record.workspace.as_deref(), Some(sibling_path.as_str()));
        assert!(artifact(&f.data, &f.ws, &sibling_id, "bep.binpb.zst").is_ok());

        let detail = crate::detail::invocation_detail(&f.data, &f.ws, &sibling_id).unwrap();
        assert!(matches!(detail.bep, crate::detail::Section::Ok { .. }));
    }

    #[test]
    fn different_repo_invocations_stay_invisible() {
        let (f, _sibling) = shared_repo_pair();
        let mut other = Fixture::new();
        other.data = f.data.clone();
        crate::workspace::test_support::repo(&other.ws, "https://example.com/other.git");
        other.complete();
        let other_id = other.stream.invocation_id.clone();

        assert_eq!(
            list(&f.data, &f.ws, std::slice::from_ref(&other_id)).unwrap(),
            list(&f.data, &f.ws, &[]).unwrap(),
        );
        assert_eq!(
            read_record(&f.data, &f.ws, &other_id).unwrap_err().kind,
            ErrorKind::Missing
        );
        assert!(artifact(&f.data, &f.ws, &other_id, "bep.binpb.zst").is_err());
        assert!(crate::detail::invocation_detail(&f.data, &f.ws, &other_id).is_err());
    }

    #[test]
    fn workspaces_without_repo_url_only_see_themselves() {
        let f = Fixture::new();
        let mut plain = Fixture::new();
        plain.data = f.data.clone();
        let mut other_plain = Fixture::new();
        other_plain.data = f.data.clone();
        crate::workspace::test_support::repo(&f.ws, "https://example.com/shared.git");
        f.complete();
        plain.complete();
        other_plain.complete();

        assert_eq!(list(&f.data, &f.ws, &[]).unwrap().len(), 1);
        let plain_list = list(&f.data, &plain.ws, &[]).unwrap();
        assert_eq!(plain_list.len(), 1);
        assert_eq!(plain_list[0].id, plain.stream.invocation_id);
        assert_eq!(
            read_record(&f.data, &plain.ws, &other_plain.stream.invocation_id)
                .unwrap_err()
                .kind,
            ErrorKind::Missing
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
        symlink(f.data.join("zelith.db"), f.data.join("zelith.db-wal")).unwrap();
        assert_eq!(
            Store::open(&f.data).err().unwrap().kind,
            ErrorKind::Permission
        );
        fs::remove_file(f.data.join("zelith.db-wal")).unwrap();
        let store = Store::open(&f.data).unwrap();
        store.conn.execute_batch("DROP TABLE workspace").unwrap();
        drop(store);
        assert_eq!(Store::open(&f.data).err().unwrap().kind, ErrorKind::Corrupt);
    }

    fn read_history_repeatedly(f: &Fixture) {
        for _ in 0..10 {
            assert_eq!(list(&f.data, &f.ws, &[]).unwrap().len(), 1);
        }
    }
}
