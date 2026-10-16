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

    let rec = store::create(&data, &ws, "build", "//pkg:a").unwrap();
    assert_eq!((rec.finished_at, rec.exit_code), (None, None));
    assert_eq!(store::read_log(&data, &ws, &rec.id).unwrap(), "");

    store::append_log(&data, &ws, &rec.id, "INFO: Analyzed target //pkg:a").unwrap();
    store::append_log(&data, &ws, &rec.id, "INFO: Build completed").unwrap();
    let done = store::finish(&data, &ws, &rec.id, 1).unwrap();
    assert_eq!(done.exit_code, Some(1));
    assert!(done.finished_at.unwrap() >= rec.started_at);

    assert_eq!(store::list(&data, &ws), vec![done]);
    assert_eq!(
        store::read_log(&data, &ws, &rec.id).unwrap(),
        "INFO: Analyzed target //pkg:a\nINFO: Build completed\n"
    );

    let json =
        std::fs::read_to_string(store::run_dir(&data, &ws, &rec.id).join("run.json")).unwrap();
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

    let old = store::create(&data, &ws, "build", "//pkg:a").unwrap();
    let old = store::finish(&data, &ws, &old.id, 0).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    // Left without finish(), like a run whose app crashed.
    let unfinished = store::create(&data, &ws, "test", "//pkg:t").unwrap();
    let broken = store::create(&data, &ws, "build", "//pkg:b").unwrap();
    std::fs::write(store::run_dir(&data, &ws, &broken.id).join("run.json"), "{").unwrap();

    let runs = store::list(&data, &ws);
    assert_eq!(runs, vec![unfinished.clone(), old]);
    assert_eq!(runs[0].finished_at, None);

    std::fs::remove_dir_all(root).unwrap();
}
