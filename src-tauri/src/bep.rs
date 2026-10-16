//! Line-by-line parse of the BEP JSON file (`--build_event_json_file`).
//!
//! Each line is one build event, keyed by the single key of its `id`. Only
//! these are read; every other event is skipped:
//!
//! - `targetCompleted`: `completed.success` → target ok, anything else failed
//! - `targetConfigured` with `aborted`: the target failed in analysis
//! - `testSummary`: `testSummary.overallStatus`
//! - `buildMetrics`: `actionSummary.actionCacheStatistics.hits`
//!
//! Bazel appends while it runs, so a killed run leaves a cut last line. The
//! first line that does not parse ends the read with `truncated = true`.

use serde::Serialize;
use serde_json::Value;
use std::io::BufRead;

#[derive(Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bep {
    pub targets: Vec<TargetResult>,
    pub tests: Vec<TestResult>,
    /// Local action cache hits. Bazel does not say which actions they were.
    pub action_cache_hits: u64,
    pub truncated: bool,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct TargetResult {
    pub label: String,
    pub ok: bool,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct TestResult {
    pub label: String,
    /// `overallStatus` as Bazel writes it, e.g. `PASSED` or `FAILED`.
    pub status: String,
}

pub fn parse<R: BufRead>(input: R) -> Bep {
    let mut bep = Bep::default();
    for line in input.lines() {
        let Some(ev) = line
            .ok()
            .and_then(|l| serde_json::from_str::<Value>(&l).ok())
        else {
            bep.truncated = true;
            break;
        };
        let id = &ev["id"];
        if let Some(t) = id.get("targetCompleted") {
            let ok = ev["completed"]["success"] == true;
            bep.targets.push(target(t, ok));
        } else if let Some(t) = id.get("targetConfigured") {
            if ev.get("aborted").is_some() {
                bep.targets.push(target(t, false));
            }
        } else if let Some(t) = id.get("testSummary") {
            bep.tests.push(TestResult {
                label: string(&t["label"]),
                status: string(&ev["testSummary"]["overallStatus"]),
            });
        } else if id.get("buildMetrics").is_some() {
            let stats = &ev["buildMetrics"]["actionSummary"]["actionCacheStatistics"];
            bep.action_cache_hits = stats["hits"].as_u64().unwrap_or(0);
        }
    }
    bep
}

fn target(id: &Value, ok: bool) -> TargetResult {
    TargetResult {
        label: string(&id["label"]),
        ok,
    }
}

fn string(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}
