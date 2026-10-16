//! `run_detail` on a run dir made by the store, with the artifacts of each
//! fixture case copied in.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use zeal_lib::{RunDetail, run_detail, store};

/// Removes the temp root when the test ends.
struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A new run dir holding `case`'s `bep.json` and `profile.gz`, if it has them.
fn run_dir(case: &str) -> (TempRoot, PathBuf) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "zeal-run-detail-{case}-{}-{nanos}",
        std::process::id()
    ));
    let ws = root.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let (_, dir) = store::create(&root.join("data"), &ws, "build", "//...").unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/run");
    for name in ["bep.json", "profile.gz"] {
        let src = fixtures.join(case).join(name);
        if src.exists() {
            fs::copy(src, dir.join(name)).unwrap();
        }
    }
    (TempRoot(root), dir)
}

fn detail(case: &str) -> RunDetail {
    let (_root, dir) = run_dir(case);
    run_detail(&dir).unwrap()
}

fn notes(d: &RunDetail) -> (Option<&str>, Option<&str>) {
    (d.bep.note.as_deref(), d.profile.note.as_deref())
}

#[test]
fn record_and_log() {
    let (_root, dir) = run_dir("ok");
    store::append_log(&dir, "INFO: Build completed").unwrap();
    let rec = store::finish(&dir, 0).unwrap();
    let d = run_detail(&dir).unwrap();
    assert_eq!(d.record, rec);
    assert_eq!(d.log, "INFO: Build completed\n");
}

#[test]
fn complete_cases_have_both_sections() {
    for case in [
        "ok",
        "disk_cache_hit",
        "action_failed",
        "analysis_failed",
        "loading_failed",
    ] {
        let d = detail(case);
        assert_eq!(notes(&d), (None, None), "{case}");
        assert!(d.bep.data.is_some() && d.profile.data.is_some(), "{case}");
    }
}

#[test]
fn bep_truncated_and_profile_missing() {
    let d = detail("bep_truncated");
    assert_eq!(notes(&d), (Some("truncated"), Some("missing")));
    assert!(!d.bep.data.unwrap().targets.is_empty());
    assert!(d.profile.data.is_none());
}

#[test]
fn broken_profile_is_a_parse_error() {
    let (_root, dir) = run_dir("ok");
    fs::write(dir.join("profile.gz"), b"not gzip").unwrap();
    let d = run_detail(&dir).unwrap();
    assert!(d.profile.data.is_none());
    assert!(d.profile.note.unwrap().starts_with("parse error: "));
}

#[test]
fn no_run_json_is_an_error() {
    let (_root, dir) = run_dir("ok");
    fs::remove_file(dir.join("run.json")).unwrap();
    assert!(run_detail(&dir).is_err());
}
