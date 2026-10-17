//! Read stored run details without confusing observation availability with
//! the child process's result.

use std::fs;
use std::path::{Path, PathBuf};
use zeal_lib::{Section, run_detail, store};

/// Removes the temp root when the test ends.
struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A new unfinished run with a fixture's observations, if present.
fn run_dir(case: &str) -> (TempRoot, PathBuf) {
    let root = std::env::temp_dir().join(format!("zeal-run-detail-{}", uuid::Uuid::new_v4()));
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

#[test]
fn record_and_log() {
    let (_root, dir) = run_dir("ok");
    store::append_log(&dir, "INFO: Build completed").unwrap();
    let rec = store::finish(&dir, 0).unwrap();
    let d = run_detail(&dir).unwrap();
    assert_eq!(d.record, rec);
    let json = serde_json::to_value(&d).unwrap();
    assert_eq!(json["record"]["exitCode"], 0);
    assert_eq!(json["log"]["status"], "ok");
    assert_eq!(json["log"]["data"], "INFO: Build completed\n");
    assert_eq!(json["profile"]["status"], "ok");
    assert!(matches!(d.log, Section::Ok { data } if data == "INFO: Build completed\n"));
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
        let (_root, dir) = run_dir(case);
        store::finish(&dir, 0).unwrap();
        let d = run_detail(&dir).unwrap();
        assert!(matches!(d.bep, Section::Ok { .. }), "{case}");
        assert!(matches!(d.profile, Section::Ok { .. }), "{case}");
    }
}

#[test]
fn incomplete_bep_and_missing_profile_preserve_the_exit_result() {
    let (_root, dir) = run_dir("bep_truncated");
    store::finish(&dir, 1).unwrap();
    let d = run_detail(&dir).unwrap();
    assert_eq!(d.record.exit_code, Some(1));
    assert!(matches!(d.bep, Section::Incomplete { data, note }
        if !data.targets.is_empty() && !note.is_empty()));
    assert!(matches!(d.profile, Section::Missing));
}

#[test]
fn complete_observations_do_not_imply_process_success() {
    let (_root, dir) = run_dir("ok");
    store::finish(&dir, 42).unwrap();
    let d = run_detail(&dir).unwrap();
    assert_eq!(d.record.exit_code, Some(42));
    assert!(matches!(d.bep, Section::Ok { .. }));
    assert!(matches!(d.profile, Section::Ok { .. }));
}

#[test]
fn unfinished_profile_is_not_parsed_even_when_it_is_valid() {
    let (_root, dir) = run_dir("ok");
    let d = run_detail(&dir).unwrap();
    assert!(matches!(d.profile, Section::NotAvailable { .. }));
    assert_eq!(d.record.finished_at, None);
    assert_eq!(d.record.exit_code, None);

    store::finish(&dir, 0).unwrap();
    let d = run_detail(&dir).unwrap();
    assert!(matches!(d.profile, Section::Ok { .. }));
}

#[test]
fn broken_profile_is_reported_only_after_completion_and_preserves_success() {
    let (_root, dir) = run_dir("ok");
    fs::write(dir.join("profile.gz"), b"not gzip").unwrap();
    assert!(matches!(
        run_detail(&dir).unwrap().profile,
        Section::NotAvailable { .. }
    ));

    store::finish(&dir, 0).unwrap();
    let d = run_detail(&dir).unwrap();
    assert_eq!(d.record.exit_code, Some(0));
    assert!(matches!(d.profile, Section::ParseError { note } if !note.is_empty()));
    assert!(matches!(d.bep, Section::Ok { .. }));
}

#[test]
fn missing_observations_do_not_hide_the_record() {
    let (_root, dir) = run_dir("ok");
    let rec = store::finish(&dir, 0).unwrap();
    fs::remove_file(dir.join("log.txt")).unwrap();
    fs::remove_file(dir.join("bep.json")).unwrap();
    fs::remove_file(dir.join("profile.gz")).unwrap();
    let d = run_detail(&dir).unwrap();
    assert_eq!(d.record, rec);
    assert!(matches!(d.log, Section::Missing));
    assert!(matches!(d.bep, Section::Missing));
    assert!(matches!(d.profile, Section::Missing));
}

#[test]
fn unreadable_log_is_reported_without_hiding_other_observations() {
    let (_root, dir) = run_dir("ok");
    store::finish(&dir, 0).unwrap();
    fs::remove_file(dir.join("log.txt")).unwrap();
    fs::create_dir(dir.join("log.txt")).unwrap();
    let d = run_detail(&dir).unwrap();
    assert!(matches!(d.log, Section::ReadError { note } if !note.is_empty()));
    assert!(matches!(d.bep, Section::Ok { .. }));
    assert!(matches!(d.profile, Section::Ok { .. }));
}

#[test]
fn each_request_reads_the_current_artifacts() {
    let (_root, dir) = run_dir("ok");
    store::finish(&dir, 0).unwrap();
    assert!(matches!(
        run_detail(&dir).unwrap().profile,
        Section::Ok { .. }
    ));
    fs::remove_file(dir.join("profile.gz")).unwrap();
    assert!(matches!(
        run_detail(&dir).unwrap().profile,
        Section::Missing
    ));
}

#[test]
fn no_run_json_is_an_error() {
    let (_root, dir) = run_dir("ok");
    fs::remove_file(dir.join("run.json")).unwrap();
    assert!(run_detail(&dir).is_err());
}
