use super::*;
use crate::store::events::test_support::Fixture;
use std::fs;
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
        assert_eq!(tables, ["run", "workspace"]);
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
fn workspace_and_safe_file_boundaries_preserve_record_when_details_disappear() {
    let f = Fixture::new();
    f.complete();
    let run = &f.stream.invocation_id;
    let other = f.root.join("other");
    fs::create_dir(&other).unwrap();
    assert!(list(&f.data, &other).unwrap().is_empty());
    assert_eq!(
        read_record(&f.data, &other, run).unwrap_err().kind,
        ErrorKind::Missing
    );
    assert!(artifact(&f.data, &other, run, "bep.binpb").is_err());
    assert!(artifact(&f.data, &f.ws, "../../escape", "bep.binpb").is_err());
    assert!(artifact(&f.data, &f.ws, run, "../zeal.db").is_err());
    let alias = f.root.join("alias");
    symlink(&f.ws, &alias).unwrap();
    assert_eq!(
        list(&f.data, &alias).unwrap(),
        list(&f.data, &f.ws).unwrap()
    );
    let path = f.dir().join("attempt/1/bep.binpb");
    fs::remove_file(&path).unwrap();
    symlink(f.data.join("zeal.db"), &path).unwrap();
    assert_eq!(
        artifact(&f.data, &f.ws, run, "bep.binpb")
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    fs::remove_dir_all(f.dir()).unwrap();
    assert_eq!(list(&f.data, &f.ws).unwrap().len(), 1);
}
#[test]
fn reservation_collision_abort_and_live_output_do_not_require_observed_workspace() {
    let f = Fixture::new();
    let run = &f.stream.invocation_id;
    let dir = reserve_invocation(&f.data, run).unwrap();
    assert_eq!(
        reserve_invocation(&f.data, run).unwrap_err().kind,
        ErrorKind::Conflict
    );
    assert!(list(&f.data, &f.ws).unwrap().is_empty());
    assert!(live_artifact(&f.data, run, "terminal.log").is_ok());
    assert_eq!(
        fs::metadata(dir.join("terminal.log"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fs::write(dir.join("unexpected"), b"keep").unwrap();
    assert!(abort_reservation(&f.data, run).is_err());
    assert!(dir.join("unexpected").exists());
    fs::remove_file(dir.join("unexpected")).unwrap();
    abort_reservation(&f.data, run).unwrap();
    assert!(!dir.exists());
    let _ = reserve_invocation(&f.data, run).unwrap();
    f.complete();
    assert_eq!(
        abort_reservation(&f.data, run).unwrap_err().kind,
        ErrorKind::Conflict
    );
}
#[test]
fn independent_readers_keep_sqlite_sidecars_safe() {
    let f = Fixture::new();
    f.complete();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let f = &f;
            scope.spawn(move || read_history_repeatedly(f));
        }
    });
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
