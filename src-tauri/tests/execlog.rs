use std::io::Cursor;
use zeal_lib::execlog::{self, ExecLog};

fn fixture(case: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/run/{case}/execlog",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn parse(bytes: &[u8]) -> ExecLog {
    execlog::parse(Cursor::new(bytes)).unwrap()
}

#[test]
fn ok_has_genrules_and_test_runner_in_sandbox() {
    let log = parse(&fixture("ok"));
    assert!(!log.truncated);

    let genrules: Vec<(&str, &str)> = log
        .actions
        .iter()
        .filter(|a| a.mnemonic == "Genrule")
        .map(|a| (a.label.as_str(), a.output.as_str()))
        .collect();
    assert_eq!(
        genrules,
        [
            ("//pkg:a", "bazel-out/k8-fastbuild/bin/pkg/a.txt"),
            ("//pkg:b", "bazel-out/k8-fastbuild/bin/pkg/b.txt"),
            ("//pkg:c", "bazel-out/k8-fastbuild/bin/pkg/c.txt"),
            ("//pkg:d", "bazel-out/k8-fastbuild/bin/pkg/d.txt"),
        ]
    );
    assert!(
        log.actions
            .iter()
            .any(|a| a.mnemonic == "TestRunner" && a.label == "//pkg:t")
    );
    assert!(
        log.actions
            .iter()
            .all(|a| a.runner == "linux-sandbox" && !a.cache_hit && a.status.is_empty())
    );
}

#[test]
fn disk_cache_hit_marks_every_action() {
    let log = parse(&fixture("disk_cache_hit"));
    assert_eq!(log.actions.len(), 4);
    assert!(
        log.actions
            .iter()
            .all(|a| a.cache_hit && a.runner == "disk cache hit")
    );
}

#[test]
fn action_failed_keeps_status() {
    let log = parse(&fixture("action_failed"));
    assert_eq!(log.actions.len(), 1);
    assert_eq!(log.actions[0].label, "//fail:x");
    assert_eq!(log.actions[0].status, "NON_ZERO_EXIT");
}

#[test]
fn analysis_failed_is_empty() {
    let log = parse(&fixture("analysis_failed"));
    assert!(log.actions.is_empty());
    assert!(!log.truncated);
}

#[test]
fn cut_inside_last_message_keeps_earlier_ones() {
    let bytes = fixture("ok");
    let full = parse(&bytes).actions;
    let log = parse(&bytes[..bytes.len() - 1]);
    assert!(log.truncated);
    assert_eq!(log.actions, full[..full.len() - 1]);
}
