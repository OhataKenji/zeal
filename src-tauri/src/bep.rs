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

pub const DISPLAY_LIMIT: usize = 8 * 1024 * 1024;
pub const LOG_LIMIT: usize = 512 * 1024;

pub struct Parsed {
    pub bep: Bep,
    pub log: String,
    pub invalid: bool,
    pub read_error: bool,
    pub events: usize,
}

/// One bounded pass builds both display sections. No event collection survives
/// a frame, and reaching the BEP display limit does not stop log extraction.
#[tracing::instrument(target = "zeal::operations", name = "detail.bep_parse", skip_all)]
pub fn parse_detail<R: Read>(mut input: R, mut remaining: u64) -> Parsed {
    let mut parsed = Parsed {
        bep: Bep::default(),
        log: String::new(),
        invalid: false,
        read_error: false,
        events: 0,
    };
    let mut display_size = json_size(&parsed.bep);
    loop {
        let mut first = [0];
        match input.read(&mut first) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                parsed.invalid = true;
                parsed.read_error = true;
                break;
            }
        }
        remaining = match remaining.checked_sub(1) {
            Some(v) => v,
            None => {
                parsed.invalid = true;
                break;
            }
        };
        let mut delimiter = CountingRead {
            inner: &mut input,
            count: 0,
            failed: false,
        };
        let length = frame_length(&mut delimiter, first[0]);
        let consumed = delimiter.count;
        parsed.read_error = delimiter.failed;
        let Some(length) = length else {
            parsed.invalid = true;
            break;
        };
        remaining = match remaining.checked_sub(consumed) {
            Some(v) => v,
            None => {
                parsed.invalid = true;
                break;
            }
        };
        if length > u64::from(MAX_FRAME_SIZE) || length > remaining {
            parsed.invalid = true;
            break;
        }
        let mut bytes = vec![0; length as usize];
        if let Err(error) = input.read_exact(&mut bytes) {
            parsed.invalid = true;
            parsed.read_error = error.kind() != io::ErrorKind::UnexpectedEof;
            break;
        }
        remaining -= length;
        let Ok(event) = BuildEvent::decode(bytes.as_slice()) else {
            parsed.invalid = true;
            break;
        };
        parsed.events += 1;
        if let Some(Payload::Progress(progress)) = &event.payload {
            append_tail(&mut parsed.log, &progress.stderr);
            append_tail(&mut parsed.log, &progress.stdout);
        }
        let mut delta = Bep::default();
        project(event, &mut delta);
        merge_bounded(&mut parsed.bep, delta, &mut display_size);
    }
    parsed.bep.truncated |= parsed.invalid;
    parsed
}

struct CountingRead<R> {
    inner: R,
    count: u64,
    failed: bool,
}
impl<R: Read> Read for CountingRead<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(bytes).inspect_err(|error| {
            self.failed = !matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof | io::ErrorKind::Interrupted
            );
        })?;
        self.count += count as u64;
        Ok(count)
    }
}
struct Size(usize);
impl io::Write for Size {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn json_size(value: &impl Serialize) -> usize {
    let mut size = Size(0);
    serde_json::to_writer(&mut size, value).expect("BEP display data serializes");
    size.0
}
fn merge_bounded(bep: &mut Bep, delta: Bep, size: &mut usize) {
    // Keep one byte for the longer `false` spelling if truncated changes.
    fn room(size: &mut usize, old: usize, new: usize) -> bool {
        let candidate = size.saturating_sub(old).saturating_add(new);
        if candidate <= DISPLAY_LIMIT {
            *size = candidate;
            true
        } else {
            false
        }
    }
    for value in delta.targets {
        if room(
            size,
            0,
            json_size(&value) + usize::from(!bep.targets.is_empty()),
        ) {
            bep.targets.push(value);
        } else {
            bep.truncated = true;
        }
    }
    for value in delta.tests {
        if room(
            size,
            0,
            json_size(&value) + usize::from(!bep.tests.is_empty()),
        ) {
            bep.tests.push(value);
        } else {
            bep.truncated = true;
        }
    }
    if let Some(options) = delta.options {
        let old = bep.options.as_ref().map_or(0, |v| json_size(v) + 11);
        if room(size, old, json_size(&options) + 11) {
            bep.options = Some(options);
        } else {
            bep.truncated = true;
        }
    }
    for (destination, incoming, key_bytes) in [
        (
            &mut bep.original_command_line,
            delta.original_command_line,
            23,
        ),
        (
            &mut bep.canonical_command_line,
            delta.canonical_command_line,
            24,
        ),
    ] {
        if let Some(value) = incoming {
            let old = destination.as_ref().map_or(0, |v| json_size(v) + key_bytes);
            if room(size, old, json_size(&value) + key_bytes) {
                *destination = Some(value);
            } else {
                bep.truncated = true;
            }
        }
    }
}
fn append_tail(log: &mut String, value: &str) {
    if value.len() >= LOG_LIMIT {
        let mut start = value.len() - LOG_LIMIT;
        while !value.is_char_boundary(start) {
            start += 1;
        }
        log.clear();
        log.push_str(&value[start..]);
    } else {
        log.push_str(value);
        let mut start = log.len().saturating_sub(LOG_LIMIT);
        while !log.is_char_boundary(start) {
            start += 1;
        }
        if start > 0 {
            log.drain(..start);
        }
    }
}
#[cfg(test)]
pub fn parse<R: Read>(input: R) -> Bep {
    parse_detail(input, u64::MAX).bep
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
    fn optional_display_fields_are_omitted_when_unreported() {
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
    fn one_scan_bounds_bep_and_unicode_log_but_keeps_logs_after_display_limit() {
        let mut stream = Vec::new();
        for _ in 0..10 {
            stream.extend(event(target(&"x".repeat(1024 * 1024), true)));
        }
        stream.extend(event(BuildEvent {
            payload: Some(Payload::OptionsParsed(
                crate::protocol::bep::OptionsParsed {
                    cmd_line: vec!["x".repeat(DISPLAY_LIMIT)],
                    ..Default::default()
                },
            )),
            ..Default::default()
        }));
        stream.extend(event(BuildEvent {
            payload: Some(Payload::Progress(crate::protocol::bep::Progress {
                stderr: "界".repeat(LOG_LIMIT),
                stdout: "tail".into(),
            })),
            ..Default::default()
        }));
        let parsed = parse_detail(stream.as_slice(), stream.len() as u64);
        assert!(!parsed.invalid);
        assert!(parsed.bep.truncated);
        assert!(serde_json::to_vec(&parsed.bep).unwrap().len() <= DISPLAY_LIMIT);
        assert!(parsed.bep.options.is_none());
        assert!(parsed.log.len() <= LOG_LIMIT);
        assert!(parsed.log.ends_with("tail"));
        assert!(parsed.log.starts_with('界'));
    }

    #[test]
    fn frame_length_cannot_allocate_past_the_file_snapshot() {
        let parsed = parse_detail([0x80, 0x80, 0x80, 0x04].as_slice(), 4);
        assert!(parsed.invalid);
        assert_eq!(parsed.events, 0);
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
