//! Golden tests for the profile parser, on the fixtures from
//! `tests/fixtures/run/gen.sh`.

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use std::fs::File;
use zeal_lib::profile::{self, Lane, Phase, Profile};

fn fixture_path(case: &str) -> String {
    format!(
        "{}/tests/fixtures/run/{case}/profile.gz",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn fixture(case: &str) -> Profile {
    profile::parse(File::open(fixture_path(case)).unwrap()).unwrap()
}

fn phase(name: &str, start_us: i64, end_us: i64) -> Phase {
    Phase {
        name: name.to_string(),
        start_us,
        end_us,
    }
}

fn lane(name: &str, tid: u64, events: Vec<Value>) -> Lane {
    Lane {
        name: name.to_string(),
        tid,
        events,
    }
}

fn lane_counts(p: &Profile) -> Vec<(&str, usize)> {
    p.lanes
        .iter()
        .map(|l| (l.name.as_str(), l.events.len()))
        .collect()
}

fn find_event<'a>(p: &'a Profile, cat: &str) -> &'a Value {
    p.lanes
        .iter()
        .flat_map(|l| l.events.iter())
        .find(|e| e["cat"] == cat)
        .unwrap()
}

/// Gzip a `traceEvents` array and parse it, for cases no fixture covers.
fn parse_events(events: Value) -> Result<Profile, String> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    serde_json::to_writer(&mut gz, &json!({ "traceEvents": events })).unwrap();
    profile::parse(gz.finish().unwrap().as_slice())
}

#[test]
fn ok() {
    let p = fixture("ok");
    assert_eq!(
        p.phases,
        [
            phase("Launch Blaze", -26000, 0),
            phase("Initialize command", 0, 2517800),
            phase("Evaluate target patterns", 2517800, 2877948),
            phase(
                "Load, analyze dependencies and build artifacts",
                2877948,
                5215357
            ),
            phase("Complete build", 5215357, 5220346),
        ]
    );
    assert_eq!(lane_counts(&p), ok_lanes());
    // Events pass through verbatim.
    assert_eq!(
        find_event(&p, "action processing"),
        &json!({
            "cat": "action processing",
            "name": "BazelWorkspaceStatusAction stable-status.txt",
            "ph": "X", "pid": 1, "tid": 191,
            "ts": 2942575, "dur": 13522,
            "out": "bazel-out/stable-status.txt",
            "args": { "mnemonic": "BazelWorkspaceStatusAction" },
        })
    );
    assert_eq!(
        find_event(&p, "critical path component"),
        &json!({
            "cat": "critical path component",
            "name": "action 'Executing genrule //pkg:a'",
            "ph": "X", "pid": 1, "tid": 0,
            "ts": 3113104, "dur": 1027091,
            "args": { "tid": 41 },
        })
    );
}

fn ok_lanes() -> Vec<(&'static str, usize)> {
    vec![
        ("Main Thread", 397),
        ("skyframe-evaluator-memoizing-0", 21),
        ("skyframe-evaluator-memoizing-1", 24),
        ("Garbage Collector", 4),
        ("skyframe-evaluator-memoizing-3", 11),
        ("skyframe-evaluator-memoizing-2", 22),
        ("skyframe-evaluator-memoizing-6", 9),
        ("skyframe-evaluator-memoizing-5", 7),
        ("skyframe-evaluator-memoizing-4", 31),
        ("skyframe-evaluator-memoizing-8", 17),
        ("skyframe-evaluator-memoizing-10", 19),
        ("skyframe-evaluator-memoizing-9", 16),
        ("skyframe-evaluator-memoizing-11", 16),
        ("skyframe-evaluator-memoizing-7", 7),
        ("skyframe-evaluator-memoizing-12", 16),
        ("skyframe-evaluator-memoizing-13", 13),
        ("skyframe-evaluator-memoizing-15", 20),
        ("skyframe-evaluator-memoizing-16", 16),
        ("skyframe-evaluator-memoizing-18", 29),
        ("skyframe-evaluator-memoizing-17", 16),
        ("skyframe-evaluator-memoizing-19", 10),
        ("skyframe-evaluator-memoizing-14", 16),
        ("skyframe-evaluator-memoizing-21", 16),
        ("skyframe-evaluator-memoizing-20", 16),
        ("skyframe-evaluator-memoizing-23", 12),
        ("skyframe-evaluator-memoizing-24", 17),
        ("skyframe-evaluator-memoizing-25", 15),
        ("skyframe-evaluator-memoizing-22", 6),
        ("skyframe-evaluator-memoizing-29", 10),
        ("skyframe-evaluator-memoizing-28", 11),
        ("skyframe-evaluator-memoizing-26", 6),
        ("skyframe-evaluator-memoizing-27", 6),
        ("skyframe-evaluator-memoizing-30", 6),
        ("skyframe-evaluator-memoizing-31", 7),
        ("skyframe-evaluator-memoizing-4", 5),
        ("skyframe-evaluator-memoizing-3", 3),
        ("skyframe-evaluator-memoizing-0", 5),
        ("skyframe-evaluator-memoizing-6", 2),
        ("skyframe-evaluator-memoizing-7", 3),
        ("skyframe-evaluator-memoizing-1", 5),
        ("skyframe-evaluator-memoizing-2", 5),
        ("skyframe-evaluator-memoizing-5", 1),
        ("tree-deleter", 9),
        ("skyframe-evaluator-5", 26),
        ("skyframe-evaluator-execution-0", 23),
        ("skyframe-evaluator-3", 21),
        ("skyframe-evaluator-7", 24),
        ("skyframe-evaluator-4", 35),
        ("skyframe-evaluator-1", 19),
        ("skyframe-evaluator-6", 35),
        ("skyframe-evaluator-2", 12),
        ("skyframe-evaluator-0", 25),
        (
            "module-extension-@@rules_kotlin+//src/main/starlark/core/repositories:bzlmod_setup.bzl%rules_kotlin_extension0 (Virtual)",
            1,
        ),
        ("skyframe-evaluator-execution-1", 23),
        (
            "starlark-repository-rules_java++toolchains+local_jd0 (Virtual)",
            5,
        ),
        ("skyframe-evaluator-execution-5", 110),
        ("skyframe-evaluator-execution-7", 8),
        ("skyframe-evaluator-execution-2", 47),
        ("skyframe-evaluator-execution-3", 8),
        ("skyframe-evaluator-execution-4", 15),
        ("Critical Path", 5),
    ]
}

#[test]
fn loading_failed() {
    let p = fixture("loading_failed");
    assert_eq!(
        p.phases,
        [
            phase("Launch Blaze", -4000, 0),
            phase("Initialize command", 0, 26972),
            phase("Evaluate target patterns", 26972, 32812),
            phase("Complete build", 32812, 33155),
        ]
    );
    assert_eq!(
        lane_counts(&p),
        [("Main Thread", 123), ("skyframe-evaluator-memoizing-3", 1)]
    );
}

#[test]
fn broken_gz_is_err() {
    let bytes = std::fs::read(fixture_path("ok")).unwrap();
    let cut = &bytes[..bytes.len() / 2];
    assert!(profile::parse(cut).is_err());
}

#[test]
fn no_finishing_is_err() {
    assert!(
        parse_events(json!([
            { "cat": "build phase marker", "name": "Launch Blaze", "ph": "X", "ts": -26000 },
        ]))
        .is_err()
    );
}

#[test]
fn events_group_by_thread() {
    let p = parse_events(json!([
        { "cat": "build phase marker", "name": "Initialize command", "ph": "X", "ts": 0 },
        { "name": "Finishing", "ph": "X", "ts": 10 },
        { "ph": "M", "name": "thread_name", "tid": 1, "args": { "name": "Main Thread" } },
        { "ph": "X", "name": "a", "cat": "general", "ts": 1, "dur": 2, "tid": 1 },
        { "ph": "C", "name": "counter", "ts": 2, "tid": 1, "args": { "n": 1 } },
        { "ph": "X", "name": "b", "cat": "action processing", "ts": 3, "dur": 1, "tid": 9 },
    ]))
    .unwrap();
    assert_eq!(
        p.lanes,
        [
            lane(
                "",
                0,
                vec![
                    json!({ "cat": "build phase marker", "name": "Initialize command", "ph": "X", "ts": 0 }),
                    json!({ "name": "Finishing", "ph": "X", "ts": 10 }),
                ]
            ),
            lane(
                "Main Thread",
                1,
                vec![
                    json!({ "ph": "X", "name": "a", "cat": "general", "ts": 1, "dur": 2, "tid": 1 }),
                    json!({ "ph": "C", "name": "counter", "ts": 2, "tid": 1, "args": { "n": 1 } }),
                ]
            ),
            lane(
                "",
                9,
                vec![
                    json!({ "ph": "X", "name": "b", "cat": "action processing", "ts": 3, "dur": 1, "tid": 9 }),
                ]
            ),
        ]
    );
}
