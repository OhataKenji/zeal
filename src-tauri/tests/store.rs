//! Round trip of the run store in a unique dir under the system temp dir.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use zeal_lib::store;

/// A fresh `(data dir, workspace dir)` pair; the caller removes the root.
fn temp_dirs(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("zeal-store-{name}-{}-{nanos}", std::process::id()));
    let (data, ws) = (root.join("data"), root.join("ws"));
    std::fs::create_dir_all(&ws).unwrap();
    (root, data, ws)
}

#[test]
fn create_log_finish_list_round_trip() {
    let (root, data, ws) = temp_dirs("round-trip");

    let (rec, dir) = store::create(&data, &ws, "build", "//pkg:a").unwrap();
    assert_eq!(dir, store::run_dir(&data, &ws, &rec.id));
    assert_eq!(rec.workspace, ws.canonicalize().unwrap().to_string_lossy());
    assert_eq!((rec.finished_at, rec.exit_code), (None, None));
    assert_eq!(store::read_log(&dir).unwrap(), "");

    store::append_log(&dir, "INFO: Analyzed target //pkg:a").unwrap();
    store::append_log(&dir, "INFO: Build completed").unwrap();
    let done = store::finish(&dir, 1).unwrap();
    assert_eq!(done.exit_code, Some(1));
    assert!(done.finished_at.unwrap() >= rec.started_at);

    assert_eq!(store::list(&data, &ws), vec![done]);
    assert_eq!(
        store::read_log(&dir).unwrap(),
        "INFO: Analyzed target //pkg:a\nINFO: Build completed\n"
    );

    let json = std::fs::read_to_string(dir.join("run.json")).unwrap();
    for key in [
        "\"id\"",
        "\"workspace\"",
        "\"cmd\"",
        "\"target\"",
        "\"startedAt\"",
        "\"finishedAt\"",
        "\"exitCode\"",
    ] {
        assert!(json.contains(key), "{key} missing in {json}");
    }

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn list_is_newest_first_with_unfinished_runs_and_skips_broken() {
    let (root, data, ws) = temp_dirs("list");

    let (_, old_dir) = store::create(&data, &ws, "build", "//pkg:a").unwrap();
    let old = store::finish(&old_dir, 0).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    // Left without finish(), like a run whose app crashed.
    let (unfinished, _) = store::create(&data, &ws, "test", "//pkg:t").unwrap();
    let (_, broken_dir) = store::create(&data, &ws, "build", "//pkg:b").unwrap();
    std::fs::write(broken_dir.join("run.json"), "{").unwrap();

    let runs = store::list(&data, &ws);
    assert_eq!(runs, vec![unfinished.clone(), old]);
    assert_eq!(runs[0].finished_at, None);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn non_ascii_workspace_round_trip() {
    let (root, data, _) = temp_dirs("non-ascii");
    let ws = root.join("開発-ワークスペース");
    std::fs::create_dir_all(&ws).unwrap();

    let (rec, dir) = store::create(&data, &ws, "build", "//pkg:a").unwrap();
    assert_eq!(dir, store::run_dir(&data, &ws, &rec.id));
    assert_eq!(rec.workspace, ws.canonicalize().unwrap().to_string_lossy());
    assert_eq!(store::list(&data, &ws), vec![rec]);

    std::fs::remove_dir_all(root).unwrap();
}

/// Unix paths may hold non-UTF-8 bytes. Two distinct workspaces whose lossy
/// strings are identical must still get distinct run dirs. (macOS rejects such
/// names at mkdir, so the dirs are never created; `run_dir` falls back to the
/// given path when canonicalize fails.)
#[cfg(unix)]
#[test]
fn non_utf8_workspaces_do_not_collide() {
    use std::os::unix::ffi::OsStrExt;

    let (root, data, _) = temp_dirs("non-utf8");
    // Both lossy-convert to "<root>/\u{FFFD}".
    let ws_a = root.join(std::ffi::OsStr::from_bytes(b"\xff"));
    let ws_b = root.join(std::ffi::OsStr::from_bytes(b"\xfe"));

    assert_ne!(
        store::run_dir(&data, &ws_a, "id"),
        store::run_dir(&data, &ws_b, "id")
    );

    std::fs::remove_dir_all(root).unwrap();
}
