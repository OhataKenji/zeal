//! Golden tests for the BEP parser, on the run fixtures from `gen.sh`.

use std::io::{BufReader, Cursor};
use zeal_lib::bep::{self, Bep, TargetResult, TestResult};

fn fixture(case: &str) -> Bep {
    let path = format!(
        "{}/tests/fixtures/run/{case}/bep.json",
        env!("CARGO_MANIFEST_DIR")
    );
    bep::parse(BufReader::new(std::fs::File::open(path).unwrap()))
}

fn target(label: &str, ok: bool) -> TargetResult {
    TargetResult {
        label: label.to_string(),
        ok,
    }
}

fn ok_targets() -> Vec<TargetResult> {
    [
        "//pkg:tool",
        "//pkg:a",
        "//pkg:c",
        "//pkg:b",
        "//pkg:d",
        "//pkg:t",
    ]
    .into_iter()
    .map(|l| target(l, true))
    .collect()
}

fn ok_tests() -> Vec<TestResult> {
    vec![TestResult {
        label: "//pkg:t".to_string(),
        status: "PASSED".to_string(),
    }]
}

#[test]
fn ok_has_all_targets_and_a_passed_test() {
    let bep = fixture("ok");
    assert_eq!(
        bep,
        Bep {
            targets: ok_targets(),
            tests: ok_tests(),
            action_cache_hits: 0,
            truncated: false,
        }
    );
}

#[test]
fn action_failed_has_the_failed_target() {
    assert_eq!(
        fixture("action_failed").targets,
        [target("//fail:x", false)]
    );
}

#[test]
fn analysis_failed_has_the_aborted_target() {
    assert_eq!(
        fixture("analysis_failed").targets,
        [target("//dep:x", false)]
    );
}

#[test]
fn loading_failed_has_no_targets() {
    let bep = fixture("loading_failed");
    assert!(bep.targets.is_empty());
    assert!(bep.tests.is_empty());
    assert!(!bep.truncated);
}

#[test]
fn truncated_keeps_the_lines_before_the_cut() {
    let bep = fixture("bep_truncated");
    assert_eq!(bep.targets, ok_targets());
    assert_eq!(bep.tests, ok_tests());
    assert!(bep.truncated);
}

#[test]
fn reads_action_cache_hits() {
    let line = r#"{"id":{"buildMetrics":{}},"buildMetrics":{"actionSummary":{"actionCacheStatistics":{"hits":12}}}}"#;
    assert_eq!(bep::parse(Cursor::new(line)).action_cache_hits, 12);
}
