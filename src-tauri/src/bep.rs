//! Bounded parse of Bazel's length-delimited protobuf BEP file.
//!
//! Target and test results, parsed options, and reported command lines form
//! the display model. The source file remains intact, including fields this
//! projection does not know about.

use crate::protocol::bep::BuildEvent;
use crate::protocol::bep::build_event::Payload;
use crate::protocol::bep::build_event_id::Id;
use crate::protocol::generated::command_line::command_line_section::SectionType;
use crate::protocol::generated::command_line::{CommandLine, Option as CommandLineOption};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_command_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_command_line: Option<String>,
    pub truncated: bool,
}

/// Project Bazel's reported original or canonical command line in section order.
pub(crate) fn project_command_line(command_line: &CommandLine, bep: &mut Bep) {
    let destination = match command_line.command_line_label.as_str() {
        "original" => &mut bep.original_command_line,
        "canonical" => &mut bep.canonical_command_line,
        _ => return,
    };
    let mut parts = Vec::new();
    for section in &command_line.sections {
        match &section.section_type {
            Some(SectionType::ChunkList(chunks)) => {
                // Bazel reports residue separately from its options. A dash
                // argument needs this boundary when rendered as a command.
                if section.section_label == "residual"
                    && chunks.chunk.first().is_some_and(|chunk| chunk != "--")
                    && chunks.chunk.iter().any(|chunk| chunk.starts_with('-'))
                {
                    parts.push("--".into());
                }
                parts.extend(chunks.chunk.iter().map(|value| quote_argument(value)));
            }
            Some(SectionType::OptionList(options)) => {
                parts.extend(options.option.iter().map(format_option));
            }
            None => {}
        }
    }
    *destination = Some(parts.join(" "));
}

/// Quote one shell argument while keeping ordinary labels and paths readable.
fn quote_argument(argument: &str) -> String {
    if !argument.is_empty()
        && argument
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_:-,.%@/=".contains(&byte))
    {
        argument.to_string()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

fn format_option(option: &CommandLineOption) -> String {
    if let Some(prefix) = option.combined_form.strip_suffix(&option.option_value) {
        // Only Bazel flag names and a separator may remain unquoted. Treat an
        // unexpected combined form as one argument instead of shell syntax.
        let name = prefix.strip_suffix('=').or_else(|| {
            let trimmed = prefix.trim_end_matches([' ', '\t']);
            (trimmed.len() < prefix.len()).then_some(trimmed)
        });
        let name = name.and_then(|name| name.strip_prefix("--").or_else(|| name.strip_prefix('-')));
        if name.is_some_and(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_:-,.%@/".contains(&byte))
        }) {
            return format!("{prefix}{}", quote_argument(&option.option_value));
        }
    }
    quote_argument(&option.combined_form)
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
    if let Some(Payload::StructuredCommandLine(command_line)) = &event.payload {
        project_command_line(command_line, bep);
    }
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
    use crate::protocol::generated::command_line::{ChunkList, CommandLineSection, OptionList};
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
        assert_eq!(parsed.original_command_line, None);
        assert_eq!(parsed.canonical_command_line, None);
    }

    fn command_line(label: &str, sections: Vec<CommandLineSection>) -> BuildEvent {
        BuildEvent {
            payload: Some(Payload::StructuredCommandLine(CommandLine {
                command_line_label: label.into(),
                sections,
            })),
            ..Default::default()
        }
    }

    fn chunks(label: &str, values: &[&str]) -> CommandLineSection {
        CommandLineSection {
            section_label: label.into(),
            section_type: Some(SectionType::ChunkList(ChunkList {
                chunk: values.iter().map(|value| (*value).into()).collect(),
            })),
        }
    }

    fn options(values: &[(&str, &str)]) -> CommandLineSection {
        CommandLineSection {
            section_label: "command options".into(),
            section_type: Some(SectionType::OptionList(OptionList {
                option: values
                    .iter()
                    .map(|(form, value)| CommandLineOption {
                        combined_form: (*form).into(),
                        option_value: (*value).into(),
                        ..Default::default()
                    })
                    .collect(),
            })),
        }
    }

    #[test]
    fn structured_command_lines_keep_order_spelling_repeats_and_argument_boundaries() {
        let mut stream = event(command_line(
            "original",
            vec![
                chunks("executable", &["bazel"]),
                options(&[("--batch", "true")]),
                chunks("command", &["test"]),
                options(&[("--define=a=b c", "a=b c"), ("--define d=e", "d=e")]),
                CommandLineSection::default(),
                chunks("residual", &["//pkg:test", ""]),
            ],
        ));
        stream.extend(event(command_line(
            "canonical",
            vec![chunks("executable", &["bazel", "test", "//pkg:test"])],
        )));
        stream.extend(event(command_line("tool", vec![])));
        let parsed = parse(stream.as_slice());
        assert_eq!(
            parsed.original_command_line.as_deref(),
            Some("bazel --batch test --define='a=b c' --define d=e //pkg:test ''")
        );
        assert_eq!(
            parsed.canonical_command_line.as_deref(),
            Some("bazel test //pkg:test")
        );
        let json = serde_json::to_value(&parsed).unwrap();
        assert_eq!(json["canonicalCommandLine"], "bazel test //pkg:test");
        assert_eq!(serde_json::from_value::<Bep>(json).unwrap(), parsed);
    }

    #[test]
    fn command_line_quoting_handles_quotes_empty_values_and_shell_syntax() {
        let parsed = parse(
            event(command_line(
                "original",
                vec![
                    chunks(
                        "executable",
                        &["/path with space/bazel", "//pkg:$(id);`whoami`"],
                    ),
                    options(&[
                        ("--run_under echo it's ready", "echo it's ready"),
                        ("-c\topt", "opt"),
                        ("--define=", ""),
                        ("--define=key=$HOME $(id)", "key=$HOME $(id)"),
                        ("--foo=$(id)=value", "value"),
                        ("--foo;echo value", "value"),
                        ("--foo\nvalue", "value"),
                        ("--foo\u{2028}value", "value"),
                        ("--foo unusual value", "different"),
                    ]),
                ],
            ))
            .as_slice(),
        );
        assert_eq!(
            parsed.original_command_line.as_deref(),
            Some(concat!(
                "'/path with space/bazel' '//pkg:$(id);`whoami`' ",
                "--run_under 'echo it'\\''s ready' -c\topt --define='' ",
                "--define='key=$HOME $(id)' '--foo=$(id)=value' '--foo;echo value' ",
                "'--foo\nvalue' '--foo\u{2028}value' '--foo unusual value'"
            ))
        );
    }

    #[test]
    fn structured_command_line_missing_empty_and_unknown_are_distinct() {
        let empty = parse(event(command_line("original", vec![])).as_slice());
        assert_eq!(empty.original_command_line.as_deref(), Some(""));
        assert_eq!(empty.canonical_command_line, None);
        let unknown = parse(event(command_line("tool", vec![])).as_slice());
        assert_eq!(unknown.original_command_line, None);
        assert_eq!(unknown.canonical_command_line, None);
        let json = serde_json::to_value(unknown).unwrap();
        assert!(json.get("originalCommandLine").is_none());
        assert!(json.get("canonicalCommandLine").is_none());
    }

    #[test]
    fn dash_residue_has_one_option_boundary() {
        let parsed = parse(
            event(command_line(
                "original",
                vec![
                    chunks("command", &["run"]),
                    chunks("residual", &["//pkg:tool", "--help"]),
                ],
            ))
            .as_slice(),
        );
        assert_eq!(
            parsed.original_command_line.as_deref(),
            Some("run -- //pkg:tool --help")
        );
        let explicit = parse(
            event(command_line(
                "original",
                vec![chunks("residual", &["--", "--help"])],
            ))
            .as_slice(),
        );
        assert_eq!(explicit.original_command_line.as_deref(), Some("-- --help"));
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
