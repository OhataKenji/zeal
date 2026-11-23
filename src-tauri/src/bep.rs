//! Bounded parse of Bazel's length-delimited protobuf BEP file.
//!
//! Target and test results, parsed options, and reported command lines form
//! the display model. The source file remains intact, including fields this
//! projection does not know about.

use crate::protocol::bep::BuildEvent;
use crate::protocol::bep::build_event::Payload;
use crate::protocol::bep::build_event_id::Id;
use crate::protocol::bep::build_metrics::{self, ActionSummary};
use crate::protocol::generated::blaze::ActionCacheStatistics;
use crate::protocol::generated::blaze::action_cache_statistics::MissReason;
use crate::protocol::generated::command_line::command_line_section::SectionType;
use crate::protocol::generated::command_line::{
    CommandLine, CommandLineSection, Option as CommandLineOption,
};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::io::{self, Read};

const MAX_FRAME_SIZE: u32 = 16 * 1024 * 1024;
// Bound decompression memory: the decoder may reference 8 MiB of history.
const MAX_WINDOW_LOG: u32 = 23;

/// One named count Bazel reports, e.g. a runner type or miss reason.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Count {
    pub name: String,
    pub count: i32,
}

/// Local action-cache counters from `action_cache_statistics`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalActionCache {
    pub hits: u32,
    pub misses: u32,
    pub miss_reasons: Vec<Count>,
}

/// Output artifacts already present on disk and reused via the local action
/// cache. The byte size crosses JSON as a decimal string so i64 values stay exact.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactCounts {
    pub count: i32,
    pub size_in_bytes: String,
}

/// Remote analysis-cache counters, also decimal strings for exact i64 values.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisCacheCounts {
    pub cache_hits: String,
    pub cache_misses: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bep {
    pub targets: Vec<TargetResult>,
    pub tests: Vec<TestSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<BuildOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_command_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_command_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_action_cache: Option<LocalActionCache>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_artifacts_from_action_cache: Option<ArtifactCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_counts: Option<Vec<Count>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_analysis_cache: Option<AnalysisCacheCounts>,
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
        append_command_line_section(section, &mut parts);
    }
    *destination = Some(parts.join(" "));
}

fn append_command_line_section(section: &CommandLineSection, parts: &mut Vec<String>) {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestSummary {
    pub label: String,
    /// `overall_status` as Bazel names it, e.g. `PASSED` or `FAILED`.
    pub status: String,
    /// `total_num_cached`; proto3 reports 0 when the field is unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_num_cached: Option<i32>,
    /// `total_run_count`; proto3 reports 0 when the field is unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_run_count: Option<i32>,
}

pub const DISPLAY_LIMIT: usize = 8 * 1024 * 1024;
pub const LOG_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub struct Parsed {
    pub bep: Bep,
    pub log: String,
    pub invalid: bool,
    pub read_error: bool,
    pub events: usize,
}

/// Decode only the saved snapshot into the bounded BEP display.
pub fn parse_zstd<R: Read>(input: R, length: u64) -> Parsed {
    let mut compressed = CountingRead {
        inner: input.take(length),
        count: 0,
        failed: false,
    };
    let decoder = zstd::stream::read::Decoder::new(&mut compressed).and_then(|mut decoder| {
        decoder.window_log_max(MAX_WINDOW_LOG)?;
        Ok(decoder)
    });
    let mut parsed = match decoder {
        Ok(decoder) => parse_detail(decoder, u64::MAX),
        Err(_) => Parsed {
            bep: Bep {
                truncated: true,
                ..Bep::default()
            },
            invalid: true,
            ..Parsed::default()
        },
    };
    // A zstd checksum/frame failure is invalid data, not a disk read failure.
    parsed.read_error &= compressed.failed;
    parsed
}

/// One bounded pass builds both display sections. No event collection survives
/// a frame, and reaching the BEP display limit does not stop log extraction.
#[tracing::instrument(target = "zelith::operations", name = "detail.bep_parse", skip_all)]
pub fn parse_detail<R: Read>(mut input: R, mut remaining: u64) -> Parsed {
    let mut parsed = Parsed::default();
    let mut display_size = json_size(&parsed.bep);
    while let Some(event) = next_event(&mut input, &mut remaining, &mut parsed) {
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

/// The next frame's event. `None` ends the scan: a clean end of stream
/// leaves `parsed.invalid` clear, anything else sets it.
fn next_event<R: Read>(
    input: &mut R,
    remaining: &mut u64,
    parsed: &mut Parsed,
) -> Option<BuildEvent> {
    let length = declared_len(input, remaining, parsed)?;
    if length > u64::from(MAX_FRAME_SIZE) || length > *remaining {
        parsed.invalid = true;
        return None;
    }
    let mut bytes = vec![0; length as usize];
    if let Err(error) = input.read_exact(&mut bytes) {
        parsed.invalid = true;
        parsed.read_error = error.kind() != io::ErrorKind::UnexpectedEof;
        return None;
    }
    *remaining -= length;
    if let Ok(event) = BuildEvent::decode(bytes.as_slice()) {
        Some(event)
    } else {
        parsed.invalid = true;
        None
    }
}

/// The next frame's declared length, its bytes charged to `remaining`.
/// `None` ends the scan as in [`next_event`].
fn declared_len<R: Read>(input: &mut R, remaining: &mut u64, parsed: &mut Parsed) -> Option<u64> {
    let mut first = [0];
    loop {
        match input.read(&mut first) {
            Ok(0) => return None,
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                parsed.invalid = true;
                parsed.read_error = true;
                return None;
            }
        }
    }
    let Some(rest) = remaining.checked_sub(1) else {
        parsed.invalid = true;
        return None;
    };
    *remaining = rest;
    let mut delimiter = CountingRead {
        inner: &mut *input,
        count: 0,
        failed: false,
    };
    let length = frame_length(&mut delimiter, first[0]);
    parsed.read_error = delimiter.failed;
    let Some(rest) = remaining.checked_sub(delimiter.count) else {
        parsed.invalid = true;
        return None;
    };
    *remaining = rest;
    if length.is_none() {
        parsed.invalid = true;
    }
    length
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
    // Push within the budget; the separator byte travels with every entry
    // after the first. `false` marks the merge truncated.
    fn push<T: Serialize>(list: &mut Vec<T>, value: T, size: &mut usize) -> bool {
        let fits = room(size, 0, json_size(&value) + usize::from(!list.is_empty()));
        if fits {
            list.push(value);
        }
        fits
    }
    // Swap in an optional field within the budget; `key_bytes` is its
    // encoded `"name":` size. `false` marks the merge truncated.
    fn replace<T: Serialize>(
        field: &mut Option<T>,
        value: Option<T>,
        key_bytes: usize,
        size: &mut usize,
    ) -> bool {
        let Some(value) = value else {
            return true;
        };
        let old = field.as_ref().map_or(0, |v| json_size(v) + key_bytes);
        let fits = room(size, old, json_size(&value) + key_bytes);
        if fits {
            *field = Some(value);
        }
        fits
    }
    for value in delta.targets {
        bep.truncated |= !push(&mut bep.targets, value, size);
    }
    for value in delta.tests {
        bep.truncated |= !push(&mut bep.tests, value, size);
    }
    bep.truncated |= !replace(&mut bep.options, delta.options, 11, size);
    bep.truncated |= !replace(
        &mut bep.original_command_line,
        delta.original_command_line,
        23,
        size,
    );
    bep.truncated |= !replace(
        &mut bep.canonical_command_line,
        delta.canonical_command_line,
        24,
        size,
    );
    bep.truncated |= !replace(
        &mut bep.local_action_cache,
        delta.local_action_cache,
        r#","localActionCache":"#.len(),
        size,
    );
    bep.truncated |= !replace(
        &mut bep.output_artifacts_from_action_cache,
        delta.output_artifacts_from_action_cache,
        r#","outputArtifactsFromActionCache":"#.len(),
        size,
    );
    bep.truncated |= !replace(
        &mut bep.runner_counts,
        delta.runner_counts,
        r#","runnerCounts":"#.len(),
        size,
    );
    bep.truncated |= !replace(
        &mut bep.remote_analysis_cache,
        delta.remote_analysis_cache,
        r#","remoteAnalysisCache":"#.len(),
        size,
    );
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

/// A breakdown count Bazel reported under a usable name. Empty names and
/// negative counts are dropped rather than mislabeled; a real zero stays.
fn named_count(name: &str, count: i32) -> Option<Count> {
    (!name.is_empty() && count >= 0).then(|| Count {
        name: name.to_owned(),
        count,
    })
}

/// Label for a BEP miss reason. An unrecognized value keeps its number
/// instead of borrowing a known reason's name.
fn miss_reason_name(reason: i32) -> String {
    match MissReason::try_from(reason) {
        Ok(reason) => reason.as_str_name().to_string(),
        Err(_) => format!("Unknown ({reason})"),
    }
}

/// Local action-cache counters. Both counts must convert; a real zero stays
/// valid while a negative value drops the whole section.
fn project_local_action_cache(stats: &ActionCacheStatistics) -> Option<LocalActionCache> {
    let hits = u32::try_from(stats.hits).ok()?;
    let misses = u32::try_from(stats.misses).ok()?;
    Some(LocalActionCache {
        hits,
        misses,
        miss_reasons: stats
            .miss_details
            .iter()
            .filter_map(|detail| named_count(&miss_reason_name(detail.reason), detail.count))
            .collect(),
    })
}

/// Runner counts in the order Bazel reported them, `total` and `internal`
/// included. `None` when no usable row was recorded.
fn project_runner_counts(summary: &ActionSummary) -> Option<Vec<Count>> {
    let counts: Vec<Count> = summary
        .runner_count
        .iter()
        .filter_map(|runner| named_count(&runner.name, runner.count))
        .collect();
    (!counts.is_empty()).then_some(counts)
}

/// Output artifacts already present on disk and reused via the local action
/// cache. Negative counters are malformed and drop the group; the byte size
/// stays a decimal string.
fn project_artifact_counts(
    metric: &build_metrics::artifact_metrics::FilesMetric,
) -> Option<ArtifactCounts> {
    (metric.count >= 0 && metric.size_in_bytes >= 0).then(|| ArtifactCounts {
        count: metric.count,
        size_in_bytes: metric.size_in_bytes.to_string(),
    })
}

/// Remote analysis-cache counters as decimal strings for exact i64 values.
fn project_analysis_cache(
    stats: &build_metrics::RemoteAnalysisCacheStatistics,
) -> Option<AnalysisCacheCounts> {
    (stats.cache_hits >= 0 && stats.cache_misses >= 0).then(|| AnalysisCacheCounts {
        cache_hits: stats.cache_hits.to_string(),
        cache_misses: stats.cache_misses.to_string(),
    })
}

/// proto3 int32 counters report 0 when unset; a negative is malformed.
fn nonnegative(value: i32) -> Option<i32> {
    (value >= 0).then_some(value)
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
    if let Some(Payload::BuildMetrics(metrics)) = &event.payload {
        if let Some(summary) = &metrics.action_summary {
            bep.local_action_cache = summary
                .action_cache_statistics
                .as_ref()
                .and_then(project_local_action_cache);
            bep.runner_counts = project_runner_counts(summary);
        }
        bep.output_artifacts_from_action_cache = metrics
            .artifact_metrics
            .as_ref()
            .and_then(|artifact| artifact.output_artifacts_from_action_cache.as_ref())
            .and_then(project_artifact_counts);
        bep.remote_analysis_cache = metrics
            .remote_analysis_cache_statistics
            .as_ref()
            .and_then(project_analysis_cache);
    }
    let Some(id) = event.id.and_then(|id| id.id) else {
        return;
    };
    match (id, event.payload) {
        (Id::TargetCompleted(id), Some(Payload::Completed(result))) => {
            bep.targets.push(TargetResult {
                label: id.label,
                ok: result.success,
                aspect: (!id.aspect.is_empty()).then_some(id.aspect),
                configuration: id
                    .configuration
                    .map(|configuration| configuration.id)
                    .filter(|id| !id.is_empty()),
            });
        }
        (Id::TargetConfigured(id), Some(Payload::Aborted(_))) => {
            bep.targets.push(TargetResult {
                label: id.label,
                ok: false,
                aspect: (!id.aspect.is_empty()).then_some(id.aspect),
                configuration: None,
            });
        }
        (Id::TestSummary(id), Some(Payload::TestSummary(summary))) => {
            bep.tests.push(TestSummary {
                label: id.label,
                status: test_status(summary.overall_status).to_string(),
                total_num_cached: nonnegative(summary.total_num_cached),
                total_run_count: nonnegative(summary.total_run_count),
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
        ConfigurationId, Id, TargetCompletedId, TargetConfiguredId, TestSummaryId,
    };
    use crate::protocol::bep::{
        Aborted, BuildEventId, TargetComplete, TestSummary as ProtoTestSummary,
    };
    use crate::protocol::generated::command_line::{ChunkList, CommandLineSection, OptionList};
    use prost::Message;

    fn event(event: BuildEvent) -> Vec<u8> {
        let mut bytes = Vec::new();
        event.encode_length_delimited(&mut bytes).unwrap();
        bytes
    }

    fn fixture(case: &str) -> Bep {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/invocation")
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

    fn completed(label: &str, aspect: &str, configuration: &str, success: bool) -> BuildEvent {
        BuildEvent {
            id: Some(BuildEventId {
                id: Some(Id::TargetCompleted(TargetCompletedId {
                    label: label.into(),
                    aspect: aspect.into(),
                    configuration: Some(ConfigurationId {
                        id: configuration.into(),
                    }),
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
    fn same_label_results_keep_their_aspect_and_configuration_identity() {
        let mut stream = event(completed("//pkg:lib", "", "cfg-a", true));
        stream.extend(event(completed("//pkg:lib", "rustfmt", "cfg-a", true)));
        stream.extend(event(completed("//pkg:lib", "clippy", "cfg-a", false)));
        stream.extend(event(completed("//pkg:lib", "", "cfg-b", true)));
        let parsed = parse(stream.as_slice());
        assert_eq!(
            parsed.targets,
            [
                TargetResult {
                    label: "//pkg:lib".into(),
                    ok: true,
                    aspect: None,
                    configuration: Some("cfg-a".into()),
                },
                TargetResult {
                    label: "//pkg:lib".into(),
                    ok: true,
                    aspect: Some("rustfmt".into()),
                    configuration: Some("cfg-a".into()),
                },
                TargetResult {
                    label: "//pkg:lib".into(),
                    ok: false,
                    aspect: Some("clippy".into()),
                    configuration: Some("cfg-a".into()),
                },
                TargetResult {
                    label: "//pkg:lib".into(),
                    ok: true,
                    aspect: None,
                    configuration: Some("cfg-b".into()),
                },
            ]
        );
        let json = serde_json::to_value(&parsed.targets).unwrap();
        assert_eq!(json[0]["configuration"], "cfg-a");
        assert!(json[0].get("aspect").is_none());
        assert_eq!(json[1]["aspect"], "rustfmt");
        assert_eq!(json[2]["aspect"], "clippy");
        assert_eq!(json[3]["configuration"], "cfg-b");
        assert!(!parsed.truncated);
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
            payload: Some(Payload::TestSummary(ProtoTestSummary {
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
                    ok: true,
                    aspect: None,
                    configuration: None,
                },
                TargetResult {
                    label: "//pkg:bad".into(),
                    ok: false,
                    aspect: None,
                    configuration: None,
                },
            ]
        );
        assert_eq!(
            parsed.tests,
            [TestSummary {
                label: "//pkg:test".into(),
                status: "FAILED".into(),
                total_num_cached: Some(0),
                total_run_count: Some(0),
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
        assert_eq!(parsed.local_action_cache, None);
        assert_eq!(parsed.output_artifacts_from_action_cache, None);
        assert_eq!(parsed.runner_counts, None);
        assert_eq!(parsed.remote_analysis_cache, None);
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
                aspect: None,
                configuration: None,
            })
        );
        assert_eq!(
            parsed.tests,
            [TestSummary {
                label: "//pkg:t".into(),
                status: "PASSED".into(),
                total_num_cached: Some(0),
                total_run_count: Some(0),
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
                aspect: None,
                configuration: None,
            }]
        );
        assert_eq!(
            fixture("analysis_failed").targets,
            [TargetResult {
                label: "//dep:x".into(),
                ok: false,
                aspect: None,
                configuration: None,
            }]
        );
    }

    #[test]
    fn empty_and_cached_fixtures_are_complete() {
        assert_eq!(fixture("loading_failed"), Bep::default());
        let cached = fixture("disk_cache_hit");
        assert_eq!(cached.local_action_cache, None);
        assert_eq!(cached.output_artifacts_from_action_cache, None);
        assert_eq!(cached.runner_counts, None);
        assert_eq!(cached.remote_analysis_cache, None);
        assert_eq!(
            cached.targets,
            [TargetResult {
                label: "//pkg:cached".into(),
                ok: true,
                aspect: None,
                configuration: None,
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
                        aspect: "ObjcProtoAspect".into(),
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
                ok: false,
                aspect: Some("ObjcProtoAspect".into()),
                configuration: None,
            }]
        );
        let json = serde_json::to_value(&parsed.targets).unwrap();
        assert_eq!(json[0]["aspect"], "ObjcProtoAspect");
        assert!(json[0].get("configuration").is_none());
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

    /// A stream that overflows the display budget, then keeps logging.
    fn over_limit_stream() -> Vec<u8> {
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
        stream
    }

    #[test]
    fn one_scan_bounds_the_bep_display_and_marks_it_truncated() {
        let stream = over_limit_stream();
        let parsed = parse_detail(stream.as_slice(), stream.len() as u64);
        assert!(!parsed.invalid);
        assert!(parsed.bep.truncated);
        assert!(serde_json::to_vec(&parsed.bep).unwrap().len() <= DISPLAY_LIMIT);
        assert!(parsed.bep.options.is_none());
    }

    #[test]
    fn the_unicode_log_tail_survives_the_display_limit() {
        let stream = over_limit_stream();
        let parsed = parse_detail(stream.as_slice(), stream.len() as u64);
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

    fn metrics_event(metrics: crate::protocol::bep::BuildMetrics) -> Vec<u8> {
        event(BuildEvent {
            payload: Some(Payload::BuildMetrics(metrics)),
            ..Default::default()
        })
    }

    fn summary_event(summary: ActionSummary) -> Vec<u8> {
        metrics_event(crate::protocol::bep::BuildMetrics {
            action_summary: Some(summary),
            ..Default::default()
        })
    }

    #[test]
    fn projects_build_metrics_cache_groups() {
        use crate::protocol::bep::build_metrics::action_summary::RunnerCount;
        use crate::protocol::bep::build_metrics::artifact_metrics::FilesMetric;
        use crate::protocol::bep::build_metrics::{ArtifactMetrics, RemoteAnalysisCacheStatistics};
        use crate::protocol::generated::blaze::action_cache_statistics::MissDetail;

        let miss = |reason: i32, count: i32| MissDetail { reason, count };
        let runner = |name: &str, count: i32| RunnerCount {
            name: name.into(),
            count,
            ..Default::default()
        };

        let parsed = parse(
            metrics_event(crate::protocol::bep::BuildMetrics {
                action_summary: Some(ActionSummary {
                    action_cache_statistics: Some(ActionCacheStatistics {
                        hits: 3,
                        misses: 1,
                        miss_details: vec![
                            miss(MissReason::DigestMismatch as i32, 1),
                            miss(MissReason::NotCached as i32, 0),
                            miss(99, 2),
                            miss(MissReason::NotCached as i32, -1),
                        ],
                        ..Default::default()
                    }),
                    runner_count: vec![
                        runner("total", 9),
                        runner("internal", 4),
                        runner("darwin-sandbox", 0),
                        runner("negative", -2),
                        runner("", 5),
                    ],
                    ..Default::default()
                }),
                artifact_metrics: Some(ArtifactMetrics {
                    output_artifacts_from_action_cache: Some(FilesMetric {
                        count: 7,
                        size_in_bytes: 9_007_199_254_740_993,
                    }),
                    ..Default::default()
                }),
                remote_analysis_cache_statistics: Some(RemoteAnalysisCacheStatistics {
                    cache_hits: 9_007_199_254_740_993,
                    cache_misses: 0,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .as_slice(),
        );
        let json = serde_json::to_value(&parsed).unwrap();
        assert_eq!(serde_json::from_value::<Bep>(json).unwrap(), parsed);
        // Producer order is kept, including `total`; unusable rows drop out.
        // i64 counters cross JSON as decimal strings.
        let groups = serde_json::json!({
            "localActionCache": parsed.local_action_cache,
            "outputArtifactsFromActionCache": parsed.output_artifacts_from_action_cache,
            "runnerCounts": parsed.runner_counts,
            "remoteAnalysisCache": parsed.remote_analysis_cache,
        });
        assert_eq!(
            groups,
            serde_json::json!({
                "localActionCache": {
                    "hits": 3,
                    "misses": 1,
                    "missReasons": [
                        {"name": "DIGEST_MISMATCH", "count": 1},
                        {"name": "NOT_CACHED", "count": 0},
                        {"name": "Unknown (99)", "count": 2},
                    ],
                },
                "outputArtifactsFromActionCache": {
                    "count": 7,
                    "sizeInBytes": "9007199254740993",
                },
                "runnerCounts": [
                    {"name": "total", "count": 9},
                    {"name": "internal", "count": 4},
                    {"name": "darwin-sandbox", "count": 0},
                ],
                "remoteAnalysisCache": {
                    "cacheHits": "9007199254740993",
                    "cacheMisses": "0",
                },
            })
        );
    }

    #[test]
    fn build_metrics_groups_stay_absent_without_records() {
        // No metrics event, empty metrics, and an empty summary all leave
        // every group absent.
        for bytes in [
            event(target("//pkg:x", true)),
            event(BuildEvent {
                payload: Some(Payload::BuildMetrics(Default::default())),
                ..Default::default()
            }),
            summary_event(ActionSummary::default()),
        ] {
            let parsed = parse(bytes.as_slice());
            assert_eq!(parsed.local_action_cache, None);
            assert_eq!(parsed.output_artifacts_from_action_cache, None);
            assert_eq!(parsed.runner_counts, None);
            assert_eq!(parsed.remote_analysis_cache, None);
        }
    }

    #[test]
    fn build_metrics_groups_keep_recorded_zeros() {
        // Recorded zeros are real data, not absence. Artifact and remote
        // counters do not depend on `action_summary`.
        let parsed = parse(
            metrics_event(crate::protocol::bep::BuildMetrics {
                artifact_metrics: Some(crate::protocol::bep::build_metrics::ArtifactMetrics {
                    output_artifacts_from_action_cache: Some(
                        crate::protocol::bep::build_metrics::artifact_metrics::FilesMetric {
                            count: 0,
                            size_in_bytes: 0,
                        },
                    ),
                    ..Default::default()
                }),
                remote_analysis_cache_statistics: Some(
                    crate::protocol::bep::build_metrics::RemoteAnalysisCacheStatistics {
                        cache_hits: 0,
                        cache_misses: 0,
                        ..Default::default()
                    },
                ),
                ..Default::default()
            })
            .as_slice(),
        );
        assert_eq!(parsed.local_action_cache, None);
        assert_eq!(
            parsed.output_artifacts_from_action_cache,
            Some(ArtifactCounts {
                count: 0,
                size_in_bytes: "0".into(),
            })
        );
        assert_eq!(parsed.runner_counts, None);
        assert_eq!(
            parsed.remote_analysis_cache,
            Some(AnalysisCacheCounts {
                cache_hits: "0".into(),
                cache_misses: "0".into(),
            })
        );
        let parsed = parse(
            summary_event(ActionSummary {
                action_cache_statistics: Some(ActionCacheStatistics {
                    hits: 0,
                    misses: 0,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .as_slice(),
        );
        assert_eq!(
            parsed.local_action_cache,
            Some(LocalActionCache {
                hits: 0,
                misses: 0,
                miss_reasons: vec![],
            })
        );
    }

    #[test]
    fn build_metrics_groups_drop_negative_counters() {
        // Negative counters are malformed and drop their group.
        for metrics in [
            crate::protocol::bep::BuildMetrics {
                action_summary: Some(ActionSummary {
                    action_cache_statistics: Some(ActionCacheStatistics {
                        hits: -1,
                        misses: 0,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            crate::protocol::bep::BuildMetrics {
                artifact_metrics: Some(crate::protocol::bep::build_metrics::ArtifactMetrics {
                    output_artifacts_from_action_cache: Some(
                        crate::protocol::bep::build_metrics::artifact_metrics::FilesMetric {
                            count: 1,
                            size_in_bytes: -1,
                        },
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            },
            crate::protocol::bep::BuildMetrics {
                remote_analysis_cache_statistics: Some(
                    crate::protocol::bep::build_metrics::RemoteAnalysisCacheStatistics {
                        cache_hits: -1,
                        cache_misses: 0,
                        ..Default::default()
                    },
                ),
                ..Default::default()
            },
        ] {
            let parsed = parse(metrics_event(metrics).as_slice());
            assert_eq!(parsed.local_action_cache, None);
            assert_eq!(parsed.output_artifacts_from_action_cache, None);
            assert_eq!(parsed.remote_analysis_cache, None);
        }
    }

    #[test]
    fn build_metrics_groups_merge_across_frames() {
        use crate::protocol::bep::build_metrics::action_summary::RunnerCount;
        use crate::protocol::generated::blaze::action_cache_statistics::MissDetail;

        let mut stream = summary_event(ActionSummary {
            action_cache_statistics: Some(ActionCacheStatistics {
                hits: 1,
                misses: 2,
                miss_details: vec![MissDetail {
                    reason: MissReason::DigestMismatch as i32,
                    count: 2,
                }],
                ..Default::default()
            }),
            runner_count: vec![RunnerCount {
                name: "internal".into(),
                count: 3,
                ..Default::default()
            }],
            ..Default::default()
        });
        stream.extend(metrics_event(crate::protocol::bep::BuildMetrics {
            artifact_metrics: Some(crate::protocol::bep::build_metrics::ArtifactMetrics {
                output_artifacts_from_action_cache: Some(
                    crate::protocol::bep::build_metrics::artifact_metrics::FilesMetric {
                        count: 4,
                        size_in_bytes: 40,
                    },
                ),
                ..Default::default()
            }),
            remote_analysis_cache_statistics: Some(
                crate::protocol::bep::build_metrics::RemoteAnalysisCacheStatistics {
                    cache_hits: 5,
                    cache_misses: 6,
                    ..Default::default()
                },
            ),
            ..Default::default()
        }));
        let parsed = parse(stream.as_slice());
        assert_eq!(
            parsed
                .local_action_cache
                .map(|local| (local.hits, local.misses)),
            Some((1, 2))
        );
        assert_eq!(
            parsed
                .runner_counts
                .as_ref()
                .map(|rows| rows[0].name.as_str()),
            Some("internal")
        );
        assert_eq!(
            parsed
                .output_artifacts_from_action_cache
                .as_ref()
                .map(|counts| counts.count),
            Some(4)
        );
        assert_eq!(
            parsed
                .remote_analysis_cache
                .as_ref()
                .map(|counts| counts.cache_hits.as_str()),
            Some("5")
        );
    }

    #[test]
    fn test_summary_reports_cached_and_attempt_counts() {
        fn summary_event_bytes(summary: ProtoTestSummary) -> Vec<u8> {
            event(BuildEvent {
                id: Some(BuildEventId {
                    id: Some(Id::TestSummary(TestSummaryId {
                        label: "//pkg:t".into(),
                        ..Default::default()
                    })),
                }),
                payload: Some(Payload::TestSummary(summary)),
                ..Default::default()
            })
        }

        let parsed = parse(
            summary_event_bytes(ProtoTestSummary {
                overall_status: 1,
                total_num_cached: 2,
                total_run_count: 24,
                ..Default::default()
            })
            .as_slice(),
        );
        assert_eq!(
            parsed.tests,
            [TestSummary {
                label: "//pkg:t".into(),
                status: "PASSED".into(),
                total_num_cached: Some(2),
                total_run_count: Some(24),
            }]
        );
        let json = serde_json::to_value(&parsed.tests[0]).unwrap();
        assert_eq!(json["totalNumCached"], 2);
        assert_eq!(json["totalRunCount"], 24);
        // Unset proto3 counters are a recorded 0; a negative is malformed.
        let parsed = parse(
            summary_event_bytes(ProtoTestSummary {
                overall_status: 1,
                ..Default::default()
            })
            .as_slice(),
        );
        assert_eq!(parsed.tests[0].total_num_cached, Some(0));
        assert_eq!(parsed.tests[0].total_run_count, Some(0));
        let parsed = parse(
            summary_event_bytes(ProtoTestSummary {
                overall_status: 1,
                total_num_cached: -3,
                ..Default::default()
            })
            .as_slice(),
        );
        assert_eq!(parsed.tests[0].total_num_cached, None);
        assert_eq!(parsed.tests[0].total_run_count, Some(0));
    }
}

#[cfg(test)]
mod zstd_tests {
    use super::*;
    use std::io::Write;

    struct FailingRead;

    impl Read for FailingRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("input read failed"))
        }
    }

    fn frame() -> Vec<u8> {
        BuildEvent {
            payload: Some(Payload::Progress(crate::protocol::bep::Progress {
                stdout: "output".into(),
                stderr: String::new(),
            })),
            ..Default::default()
        }
        .encode_length_delimited_to_vec()
    }

    /// A complete checksummed frame, matching what the receiver writes.
    fn zstd(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 0).unwrap();
        encoder.include_checksum(true).unwrap();
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn zstd_progress_is_decoded() {
        let encoded = zstd(&frame());
        let parsed = parse_zstd(encoded.as_slice(), encoded.len() as u64);
        assert_eq!(parsed.log, "output");
        assert_eq!(parsed.events, 1);
        assert!(!parsed.invalid);
    }

    #[test]
    fn snapshot_excludes_later_appends() {
        let mut encoded = zstd(&frame());
        let length = encoded.len() as u64;
        encoded.extend(b"later invalid zstd");
        let parsed = parse_zstd(encoded.as_slice(), length);
        assert_eq!(parsed.log, "output");
        assert!(!parsed.invalid);
    }

    #[test]
    fn truncated_checksum_preserves_prefix_as_invalid() {
        let frame = frame();
        let mut encoded = zstd(&frame);
        encoded.truncate(encoded.len() - 1);
        let parsed = parse_zstd(encoded.as_slice(), encoded.len() as u64);
        assert_eq!(parsed.log, "output");
        assert!(parsed.invalid);
    }

    #[test]
    fn corrupt_checksum_preserves_prefix_as_invalid() {
        let frame = frame();
        let mut encoded = zstd(&frame);
        let last = encoded.len() - 1;
        encoded[last] ^= 1;
        let parsed = parse_zstd(encoded.as_slice(), encoded.len() as u64);
        assert_eq!(parsed.log, "output");
        assert!(parsed.invalid);
    }

    #[test]
    fn physical_input_failure_is_a_read_error() {
        let parsed = parse_zstd(FailingRead, 1);
        assert!(parsed.invalid);
        assert!(parsed.read_error);
    }
}
