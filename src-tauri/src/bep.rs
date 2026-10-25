//! Line-by-line parse of the BEP JSON file (`--build_event_json_file`).
//!
//! Each line is one build event, keyed by the single key of its `id`. Only
//! these are read; every other event is skipped:
//!
//! - `targetCompleted`: `completed.success` → target ok, anything else failed
//! - `targetConfigured` with `aborted`: the target failed in analysis
//! - `testSummary`: `testSummary.overallStatus`
//!
//! Bazel appends while it runs, so a killed run leaves a cut last line. The
//! first line that does not parse ends the read with `truncated = true`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::BufRead;

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bep {
    pub targets: Vec<TargetResult>,
    pub tests: Vec<TestResult>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct TargetResult {
    pub label: String,
    pub ok: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct TestResult {
    pub label: String,
    /// `overallStatus` as Bazel writes it, e.g. `PASSED` or `FAILED`.
    pub status: String,
}

#[tracing::instrument(target = "zeal::operations", name = "detail.bep_parse", skip_all)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn fixture(case: &str) -> Bep {
        let path = format!(
            "{}/tests/fixtures/run/{case}/bep.json",
            env!("CARGO_MANIFEST_DIR")
        );
        parse(BufReader::new(std::fs::File::open(path).unwrap()))
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
}
