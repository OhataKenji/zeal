//! Bounded reads of the saved Bazel profiling trace. The gzip artifact is
//! handed to the bundled Perfetto viewer verbatim, and a streaming pass over
//! the same handle extracts the top critical-path steps without materializing
//! the full trace in memory.
use std::cmp::Ordering;
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::store;

/// Compressed artifact bound. The viewer ingests the gzip directly, so the
/// expanded JSON has no Rust-side transfer limit.
pub const COMPRESSED_LIMIT: u64 = 64 * 1024 * 1024;
const EXPANDED_SCAN_LIMIT: u64 = 128 * 1024 * 1024;
const CRITICAL_PATH_LIMIT: usize = 5;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CriticalPathStep {
    pub name: String,
    pub duration_us: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CriticalPathSummary {
    pub total_us: f64,
    pub steps: Vec<CriticalPathStep>,
}

#[derive(Deserialize)]
struct TraceArgs {
    target: Option<String>,
    out: Option<String>,
}

#[derive(Deserialize)]
struct TraceEvent {
    cat: Option<String>,
    name: Option<String>,
    ts: Option<f64>,
    dur: Option<f64>,
    out: Option<String>,
    args: Option<TraceArgs>,
}

struct RawStep {
    step: CriticalPathStep,
    ts: f64,
}

fn read_trace_events<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<Vec<RawStep>, A::Error> {
    let mut steps = Vec::new();
    while let Some(ev) = seq.next_element::<TraceEvent>()? {
        if ev.cat.as_deref() != Some("critical path component") {
            continue;
        }
        let Some(dur) = ev.dur.filter(|d| d.is_finite() && *d > 0.0) else {
            continue;
        };
        let raw_name = ev.name.unwrap_or_default();
        let name = raw_name
            .strip_prefix("action '")
            .and_then(|s| s.strip_suffix('\''))
            .unwrap_or(&raw_name)
            .to_string();
        let (target, args_out) = match ev.args {
            Some(a) => (a.target, a.out),
            None => (None, None),
        };
        let out = ev.out.or(args_out);
        steps.push(RawStep {
            step: CriticalPathStep {
                name,
                duration_us: dur,
                target,
                out,
            },
            ts: ev.ts.filter(|t| t.is_finite()).unwrap_or(0.0),
        });
    }
    Ok(steps)
}

struct TraceEventsSeed;
struct SeqVisitor;

impl<'de> Visitor<'de> for SeqVisitor {
    type Value = Vec<RawStep>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a sequence of trace events")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        read_trace_events(seq)
    }
}

impl<'de> DeserializeSeed<'de> for TraceEventsSeed {
    type Value = Vec<RawStep>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(SeqVisitor)
    }
}

struct TraceRoot(Vec<RawStep>);
struct RootVisitor;

fn read_trace_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<Vec<RawStep>, A::Error> {
    let mut steps = Vec::new();
    while let Some(key) = map.next_key::<String>()? {
        if key == "traceEvents" {
            steps = map.next_value_seed(TraceEventsSeed)?;
        } else {
            map.next_value::<IgnoredAny>()?;
        }
    }
    Ok(steps)
}

impl<'de> Visitor<'de> for RootVisitor {
    type Value = TraceRoot;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a Bazel JSON trace object or array")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        Ok(TraceRoot(read_trace_events(seq)?))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        Ok(TraceRoot(read_trace_map(map)?))
    }
}

impl<'de> Deserialize<'de> for TraceRoot {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(RootVisitor)
    }
}

/// Extract the top critical-path steps from an open `profile.json.gz` handle
/// and rewind the handle to byte 0 so Perfetto can still stream it verbatim.
/// Returns an empty summary on malformed or oversized expanded traces.
pub fn critical_path(file: &mut File) -> io::Result<CriticalPathSummary> {
    let summary = critical_path_from_reader(BufReader::new(file.take(COMPRESSED_LIMIT)));
    file.seek(SeekFrom::Start(0))?;
    Ok(summary)
}

fn critical_path_from_reader(input: impl Read) -> CriticalPathSummary {
    let decoder = BufReader::new(GzDecoder::new(input).take(EXPANDED_SCAN_LIMIT));
    let Ok(TraceRoot(mut raw)) = serde_json::from_reader(decoder) else {
        return CriticalPathSummary::default();
    };
    let total_us = raw.iter().map(|item| item.step.duration_us).sum();
    raw.sort_by(|a, b| {
        b.step
            .duration_us
            .partial_cmp(&a.step.duration_us)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.ts.partial_cmp(&b.ts).unwrap_or(Ordering::Equal))
    });
    raw.truncate(CRITICAL_PATH_LIMIT);
    CriticalPathSummary {
        total_us,
        steps: raw.into_iter().map(|item| item.step).collect(),
    }
}

/// Open the saved trace artifact for a one-shot grant, refusing anything past
/// the compressed bound before the handle is issued. Metadata rejects
/// oversized files before any allocation; holding the `File` keeps the read
/// pinned to the validated inode.
pub fn open(data: &Path, workspace: &Path, id: &str) -> io::Result<File> {
    let file = store::artifact(data, workspace, id, "profile.json.gz")?;
    if file.metadata()?.len() > COMPRESSED_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile exceeds the 64 MiB limit",
        ));
    }
    Ok(file)
}

/// Read a granted artifact. The bounded read stays as a backstop if the file
/// grew past the limit after `open`.
pub fn read(file: File) -> io::Result<Vec<u8>> {
    trace_bytes(file, COMPRESSED_LIMIT)
}

fn trace_bytes(input: impl Read, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile exceeds the transfer size limit",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::events::test_support::Fixture;

    fn fixture_with_profile(bytes: &[u8]) -> (Fixture, String) {
        let f = Fixture::new();
        f.complete();
        let dir = f.dir().join("attempt/1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("profile.json.gz"), bytes).unwrap();
        let id = f.stream.invocation_id.clone();
        (f, id)
    }

    #[test]
    fn open_and_read_return_the_artifact_bytes_unchanged() {
        let expected = [0x1f, 0x8b, 0xff, 0x00];
        let (f, id) = fixture_with_profile(&expected);
        let file = open(&f.data, &f.ws, &id).unwrap();
        assert_eq!(read(file).unwrap(), expected);
    }

    #[test]
    fn open_rejects_an_over_cap_artifact() {
        // Sparse file: the metadata bound trips on length, not allocation.
        let (f, id) = fixture_with_profile(b"gzip");
        let path = f.dir().join("attempt/1/profile.json.gz");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(COMPRESSED_LIMIT + 1).unwrap();
        assert!(open(&f.data, &f.ws, &id).is_err());
    }

    #[test]
    fn open_fails_closed_without_an_artifact() {
        let f = Fixture::new();
        f.complete();
        let id = f.stream.invocation_id.clone();
        assert_eq!(
            open(&f.data, &f.ws, &id).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn trace_bytes_stops_one_past_the_bound() {
        // Reads up to `limit` bytes; one byte more fails as InvalidData.
        assert_eq!(trace_bytes(&b"abc"[..], 3).unwrap(), b"abc");
        assert_eq!(
            trace_bytes(&b"abcd"[..], 3).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    fn gzip(json: &str) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(json.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn critical_path_ranks_top_five_sums_all_and_rewinds_file() {
        let trace = serde_json::json!({
            "otherData": { "build_id": "1" },
            "traceEvents": [
                { "cat": "action processing", "name": "Ignored", "ts": 1, "dur": 9999 },
                { "cat": "critical path component", "name": "action 'Step 1'", "ts": 10, "dur": 100 },
                { "cat": "critical path component", "name": "action 'Step 2'", "ts": 20, "dur": 600, "out": "bazel-out/k8-fastbuild/bin/rates/rate_server", "args": { "target": "//rates:rate_server" } },
                { "cat": "critical path component", "name": "Step 3", "ts": 30, "dur": 300 },
                { "cat": "critical path component", "name": "action 'Step 4'", "ts": 40, "dur": 300 },
                { "cat": "critical path component", "name": "action 'Step 5'", "ts": 50, "dur": 500 },
                { "cat": "critical path component", "name": "action 'Step 6'", "ts": 60, "dur": 200 },
                { "cat": "critical path component", "name": "Zero", "ts": 70, "dur": 0 }
            ]
        });
        let gz = gzip(&trace.to_string());
        let (f, id) = fixture_with_profile(&gz);
        let mut file = open(&f.data, &f.ws, &id).unwrap();
        let summary = critical_path(&mut file).unwrap();
        assert_eq!(summary.total_us, 2000.0);
        assert_eq!(summary.steps.len(), 5);
        assert_eq!(summary.steps[0].name, "Step 2");
        assert_eq!(
            summary.steps[0].target.as_deref(),
            Some("//rates:rate_server")
        );
        assert_eq!(
            summary.steps[0].out.as_deref(),
            Some("bazel-out/k8-fastbuild/bin/rates/rate_server")
        );
        assert_eq!(
            summary
                .steps
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Step 2", "Step 5", "Step 3", "Step 4", "Step 6"]
        );
        // Handle was rewound so Perfetto still reads the intact gzip bytes.
        assert_eq!(read(file).unwrap(), gz);
    }

    #[test]
    fn critical_path_returns_empty_on_corrupt_gzip() {
        let (f, id) = fixture_with_profile(b"not-a-gzip");
        let mut file = open(&f.data, &f.ws, &id).unwrap();
        assert_eq!(
            critical_path(&mut file).unwrap(),
            CriticalPathSummary::default()
        );
        assert_eq!(read(file).unwrap(), b"not-a-gzip");
    }
}
