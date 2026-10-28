//! Bounded parse of Bazel's length-delimited protobuf BEP file.
//!
//! Only target completion, aborted configuration, and test summary events
//! contribute to the existing display model. The source file remains intact,
//! including fields this projection does not know about.

use crate::protocol::bep::BuildEvent;
use crate::protocol::bep::build_event::Payload;
use crate::protocol::bep::build_event_id::Id;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::io::{self, Read};

const MAX_FRAME_SIZE: u32 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bep {
    pub targets: Vec<TargetResult>,
    pub tests: Vec<TestResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<BuildOptions>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildOptions {
    pub startup_options: Vec<String>,
    pub explicit_startup_options: Vec<String>,
    pub cmd_line: Vec<String>,
    pub explicit_cmd_line: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct TargetResult {
    pub label: String,
    pub ok: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct TestResult {
    pub label: String,
    /// `overall_status` as Bazel names it, e.g. `PASSED` or `FAILED`.
    pub status: String,
}

#[tracing::instrument(target = "zeal::operations", name = "detail.bep_parse", skip_all)]
pub fn parse<R: Read>(mut input: R) -> Bep {
    let mut bep = Bep::default();
    loop {
        let mut first = [0];
        match input.read(&mut first) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                bep.truncated = true;
                break;
            }
        }
        let Some(length) = frame_length(&mut input, first[0]) else {
            bep.truncated = true;
            break;
        };
        if length > u64::from(MAX_FRAME_SIZE) {
            bep.truncated = true;
            break;
        }
        let mut bytes = vec![0; length as usize];
        if input.read_exact(&mut bytes).is_err() {
            bep.truncated = true;
            break;
        }
        let Ok(event) = BuildEvent::decode(bytes.as_slice()) else {
            bep.truncated = true;
            break;
        };
        project(event, &mut bep);
    }
    bep
}

/// Read a protobuf uint64 delimiter. `None` means incomplete or malformed.
fn frame_length<R: Read>(input: &mut R, first: u8) -> Option<u64> {
    let mut value = u64::from(first & 0x7f);
    if first & 0x80 == 0 {
        return Some(value);
    }
    for index in 1..10 {
        let mut byte = [0];
        input.read_exact(&mut byte).ok()?;
        let byte = byte[0];
        if index == 9 && byte & 0xfe != 0 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

pub(crate) fn test_status(status: i32) -> &'static str {
    match status {
        1 => "PASSED",
        2 => "FLAKY",
        3 => "TIMEOUT",
        4 => "FAILED",
        5 => "INCOMPLETE",
        6 => "REMOTE_FAILURE",
        7 => "FAILED_TO_BUILD",
        8 => "TOOL_HALTED_BEFORE_TESTING",
        _ => "NO_STATUS",
    }
}

fn project(event: BuildEvent, bep: &mut Bep) {
    if let Some(Payload::OptionsParsed(options)) = &event.payload {
        bep.options = Some(BuildOptions {
            startup_options: options.startup_options.clone(),
            explicit_startup_options: options.explicit_startup_options.clone(),
            cmd_line: options.cmd_line.clone(),
            explicit_cmd_line: options.explicit_cmd_line.clone(),
        });
    }
    let Some(id) = event.id.and_then(|id| id.id) else {
        return;
    };
    match (id, event.payload) {
        (Id::TargetCompleted(id), Some(Payload::Completed(result))) => {
            bep.targets.push(TargetResult {
                label: id.label,
                ok: result.success,
            });
        }
        (Id::TargetConfigured(id), Some(Payload::Aborted(_))) => {
            bep.targets.push(TargetResult {
                label: id.label,
                ok: false,
            });
        }
        (Id::TestSummary(id), Some(Payload::TestSummary(summary))) => {
            bep.tests.push(TestResult {
                label: id.label,
                status: test_status(summary.overall_status).to_string(),
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::bep::build_event::Payload;
    use crate::protocol::bep::build_event_id::{
        Id, TargetCompletedId, TargetConfiguredId, TestSummaryId,
    };
    use crate::protocol::bep::{Aborted, BuildEventId, TargetComplete, TestSummary};
    use prost::Message;

    fn event(event: BuildEvent) -> Vec<u8> {
        let mut bytes = Vec::new();
        event.encode_length_delimited(&mut bytes).unwrap();
        bytes
    }

    fn fixture(case: &str) -> Bep {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/run")
            .join(case)
            .join("bep.binpb");
        parse(std::fs::File::open(path).unwrap())
    }

    fn target(label: &str, success: bool) -> BuildEvent {
        BuildEvent {
            id: Some(BuildEventId {
                id: Some(Id::TargetCompleted(TargetCompletedId {
                    label: label.into(),
                    ..Default::default()
                })),
            }),
            payload: Some(Payload::Completed(TargetComplete {
                success,
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    #[test]
    fn projects_target_results_and_test_statuses() {
        let mut stream = event(target("//pkg:ok", true));
        stream.extend(event(target("//pkg:bad", false)));
        stream.extend(event(BuildEvent {
            id: Some(BuildEventId {
                id: Some(Id::TestSummary(TestSummaryId {
                    label: "//pkg:test".into(),
                    ..Default::default()
                })),
            }),
            payload: Some(Payload::TestSummary(TestSummary {
                overall_status: 4,
                ..Default::default()
            })),
            ..Default::default()
        }));
        let parsed = parse(stream.as_slice());
        assert_eq!(
            parsed.targets,
            [
                TargetResult {
                    label: "//pkg:ok".into(),
                    ok: true
                },
                TargetResult {
                    label: "//pkg:bad".into(),
                    ok: false
                },
            ]
        );
        assert_eq!(
            parsed.tests,
            [TestResult {
                label: "//pkg:test".into(),
                status: "FAILED".into()
            }]
        );
        assert!(!parsed.truncated);
    }

    #[test]
    fn projects_all_options_parsed_groups_and_keeps_empty_distinct_from_missing() {
        let parsed = parse(
            event(BuildEvent {
                payload: Some(Payload::OptionsParsed(
                    crate::protocol::bep::OptionsParsed {
                        startup_options: vec!["--batch".into(), "--output_base=/tmp/base".into()],
                        explicit_startup_options: vec!["--batch".into()],
                        cmd_line: vec!["--color=yes".into(), "--jobs=8".into()],
                        explicit_cmd_line: vec!["--jobs=8".into()],
                        ..Default::default()
                    },
                )),
                ..Default::default()
            })
            .as_slice(),
        );
        assert_eq!(
            parsed.options,
            Some(BuildOptions {
                startup_options: vec!["--batch", "--output_base=/tmp/base"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                explicit_startup_options: vec!["--batch".into()],
                cmd_line: vec!["--color=yes".into(), "--jobs=8".into()],
                explicit_cmd_line: vec!["--jobs=8".into()],
            })
        );
        assert_eq!(parse(event(BuildEvent::default()).as_slice()).options, None);
        assert_eq!(
            parse(
                event(BuildEvent {
                    payload: Some(Payload::OptionsParsed(Default::default())),
                    ..Default::default()
                })
                .as_slice()
            )
            .options,
            Some(BuildOptions::default())
        );
    }

    #[test]
    fn old_bep_json_without_options_still_deserializes() {
        let parsed: Bep =
            serde_json::from_str(r#"{"targets":[],"tests":[],"truncated":false}"#).unwrap();
        assert_eq!(parsed.options, None);
    }

    #[test]
    fn ok_fixture_has_all_expected_targets_and_test() {
        let parsed = fixture("ok");
        assert_eq!(
            parsed.targets,
            [
                "//pkg:tool",
                "//pkg:a",
                "//pkg:c",
                "//pkg:b",
                "//pkg:d",
                "//pkg:t"
            ]
            .map(|label| TargetResult {
                label: label.into(),
                ok: true,
            })
        );
        assert_eq!(
            parsed.tests,
            [TestResult {
                label: "//pkg:t".into(),
                status: "PASSED".into(),
            }]
        );
        assert!(!parsed.truncated);
    }

    #[test]
    fn failed_and_aborted_fixtures_keep_the_failed_target() {
        assert_eq!(
            fixture("action_failed").targets,
            [TargetResult {
                label: "//fail:x".into(),
                ok: false,
            }]
        );
        assert_eq!(
            fixture("analysis_failed").targets,
            [TargetResult {
                label: "//dep:x".into(),
                ok: false,
            }]
        );
    }

    #[test]
    fn empty_and_cached_fixtures_are_complete() {
        assert_eq!(fixture("loading_failed"), Bep::default());
        assert_eq!(
            fixture("disk_cache_hit").targets,
            [TargetResult {
                label: "//pkg:cached".into(),
                ok: true,
            }]
        );
    }

    #[test]
    fn truncated_fixture_retains_completed_targets_and_test() {
        let parsed = fixture("bep_truncated");
        let complete = fixture("ok");
        assert_eq!(parsed.targets, complete.targets);
        assert_eq!(parsed.tests, complete.tests);
        assert!(parsed.truncated);
    }

    #[test]
    fn aborted_configuration_is_a_failed_target() {
        let parsed = parse(
            event(BuildEvent {
                id: Some(BuildEventId {
                    id: Some(Id::TargetConfigured(TargetConfiguredId {
                        label: "//dep:x".into(),
                        ..Default::default()
                    })),
                }),
                payload: Some(Payload::Aborted(Aborted::default())),
                ..Default::default()
            })
            .as_slice(),
        );
        assert_eq!(
            parsed.targets,
            [TargetResult {
                label: "//dep:x".into(),
                ok: false
            }]
        );
    }

    #[test]
    fn clean_boundary_eof_is_complete() {
        assert_eq!(parse([].as_slice()), Bep::default());
        assert!(!parse(event(target("//pkg:x", true)).as_slice()).truncated);
        assert!(!parse([0x80, 0].as_slice()).truncated);
    }

    #[test]
    fn partial_varint_keeps_previous_events_and_marks_incomplete() {
        let mut stream = event(target("//pkg:x", true));
        stream.push(0x80);
        assert_eq!(parse(stream.as_slice()).targets.len(), 1);
        assert!(parse(stream.as_slice()).truncated);
    }

    #[test]
    fn partial_body_keeps_previous_events_and_marks_incomplete() {
        let mut stream = event(target("//pkg:x", true));
        stream.extend([0x03, 0x0a]);
        assert_eq!(parse(stream.as_slice()).targets.len(), 1);
        assert!(parse(stream.as_slice()).truncated);
    }

    #[test]
    fn malformed_frame_keeps_previous_events_and_marks_incomplete() {
        let mut stream = event(target("//pkg:x", true));
        stream.extend([0x01, 0xff]);
        assert_eq!(parse(stream.as_slice()).targets.len(), 1);
        assert!(parse(stream.as_slice()).truncated);
    }

    #[test]
    fn rejects_overflowing_and_oversize_frame_lengths() {
        for prefix in [
            vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02],
            vec![0x82, 0x80, 0x80, 0x08],
        ] {
            let parsed = parse(prefix.as_slice());
            assert!(parsed.truncated);
            assert!(parsed.targets.is_empty());
        }
    }

    #[test]
    fn test_status_names_match_bazel() {
        assert_eq!(
            (0..=8).map(test_status).collect::<Vec<_>>(),
            [
                "NO_STATUS",
                "PASSED",
                "FLAKY",
                "TIMEOUT",
                "FAILED",
                "INCOMPLETE",
                "REMOTE_FAILURE",
                "FAILED_TO_BUILD",
                "TOOL_HALTED_BEFORE_TESTING",
            ]
        );
    }
}
