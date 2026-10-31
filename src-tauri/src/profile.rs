//! Bounded Bazel gzip trace parsing. Raw event fields and lane ordering remain intact.
use flate2::read::MultiGzDecoder;
use serde::Serialize;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const COMPRESSED_LIMIT: usize = 32 * 1024 * 1024;
pub const EXPANDED_LIMIT: usize = 64 * 1024 * 1024;
pub const EVENT_LIMIT: usize = 250_000;
pub const IPC_LIMIT: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub phases: Vec<Phase>,
    pub lanes: Vec<Lane>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Phase {
    pub name: String,
    pub start_us: i64,
    pub end_us: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lane {
    pub name: String,
    pub tid: u64,
    pub events: Vec<Value>,
}

#[derive(Debug)]
pub enum ProfileError {
    Incomplete(String),
    Parse(String),
}

/// Checks before exposing another byte to the parser. The extra byte only
/// determines overflow; it is never retained or deserialized.
struct Limited<R> {
    input: R,
    remaining: usize,
    exceeded: Arc<AtomicBool>,
}
impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining > 0 {
            let size = buf.len().min(self.remaining);
            let n = self.input.read(&mut buf[..size])?;
            self.remaining -= n;
            return Ok(n);
        }
        let mut byte = [0];
        if self.input.read(&mut byte)? == 0 {
            return Ok(0);
        }
        self.exceeded.store(true, Ordering::Relaxed);
        Err(io::Error::other("profile byte limit exceeded"))
    }
}

/// Counts serialized JSON without allocating a second copy of each event.
struct JsonBudget {
    remaining: usize,
}
impl Write for JsonBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len())
            .ok_or_else(|| io::Error::other("profile IPC limit exceeded"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl JsonBudget {
    fn charge<T: Serialize>(&mut self, value: &T) -> Result<(), String> {
        serde_json::to_writer(self, value).map_err(|_| "profile IPC limit exceeded".to_string())
    }
    fn overhead(&mut self, bytes: usize) -> Result<(), String> {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or("profile IPC limit exceeded")?;
        Ok(())
    }
}
fn str_of<'a>(event: &'a Value, key: &str) -> Option<&'a str> {
    event.get(key).and_then(Value::as_str)
}

struct Collector {
    profile: Profile,
    lane_of: HashMap<u64, usize>,
    names: HashMap<u64, String>,
    finishing: Option<i64>,
    event_count: usize,
    event_limit: usize,
    budget: JsonBudget,
    incomplete: bool,
}
impl Collector {
    fn event(&mut self, event: Value) -> Result<(), String> {
        let result = self.collect_event(event);
        self.incomplete |= result.is_err();
        result
    }

    fn collect_event(&mut self, event: Value) -> Result<(), String> {
        self.event_count += 1;
        if self.event_count > self.event_limit {
            return Err("profile event limit exceeded".to_string());
        }
        // Charge raw fields, lane and phase overhead before storing anything.
        // Charging metadata too bounds metadata retained before the first event.
        self.budget.charge(&event)?;
        self.budget.overhead(1)?;
        if str_of(&event, "cat") == Some("build phase marker") {
            let name = str_of(&event, "name").unwrap_or_default().to_string();
            self.budget.charge(&name)?;
            self.budget.overhead(96)?;
            self.profile.phases.push(Phase {
                name,
                start_us: event["ts"].as_i64().unwrap_or(0),
                end_us: 0,
            });
        }
        if self.finishing.is_none() && str_of(&event, "name") == Some("Finishing") {
            self.finishing = event["ts"].as_i64();
        }
        let tid = event["tid"].as_u64().unwrap_or(0);
        if str_of(&event, "ph") == Some("M") {
            self.record_thread_name(tid, &event);
            return Ok(());
        }
        let i = if let Some(i) = self.lane_of.get(&tid) {
            *i
        } else {
            self.budget.overhead(96)?;
            let i = self.profile.lanes.len();
            self.profile.lanes.push(Lane {
                name: String::new(),
                tid,
                events: Vec::new(),
            });
            self.lane_of.insert(tid, i);
            i
        };
        self.profile.lanes[i].events.push(event);
        Ok(())
    }
    fn record_thread_name(&mut self, tid: u64, event: &Value) {
        if str_of(event, "name") == Some("thread_name")
            && let Some(name) = event["args"]["name"].as_str()
        {
            self.names.insert(tid, name.to_string());
        }
    }

    fn finish(mut self) -> Result<Profile, ProfileError> {
        let finishing = self
            .finishing
            .ok_or_else(|| ProfileError::Parse("no Finishing event".to_string()))?;
        self.profile.phases.sort_by_key(|phase| phase.start_us);
        for i in 0..self.profile.phases.len() {
            self.profile.phases[i].end_us = self
                .profile
                .phases
                .get(i + 1)
                .map_or(finishing, |phase| phase.start_us);
        }
        for lane in &mut self.profile.lanes {
            lane.name = self.names.remove(&lane.tid).unwrap_or_default();
        }
        Ok(self.profile)
    }
}

struct Events<'a>(&'a mut Collector);
impl<'de> DeserializeSeed<'de> for Events<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for Events<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("trace event array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(event) = seq.next_element::<Value>()? {
            self.0.event(event).map_err(de::Error::custom)?;
        }
        Ok(())
    }
}
struct Trace<'a> {
    collector: &'a mut Collector,
    found_events: bool,
}

impl Trace<'_> {
    fn read_field<'de, A: MapAccess<'de>>(
        &mut self,
        key: &str,
        map: &mut A,
    ) -> Result<(), A::Error> {
        if key != "traceEvents" {
            map.next_value::<IgnoredAny>()?;
            return Ok(());
        }
        if self.found_events {
            return Err(de::Error::duplicate_field("traceEvents"));
        }
        self.found_events = true;
        map.next_value_seed(Events(self.collector))
    }
}

impl<'de> Visitor<'de> for Trace<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("trace object")
    }
    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            self.read_field(&key, &mut map)?;
        }
        if !self.found_events {
            return Err(de::Error::missing_field("traceEvents"));
        }
        Ok(())
    }
}

fn parse_limits<R: Read>(
    input: R,
    compressed: usize,
    expanded: usize,
    events: usize,
    budget: usize,
) -> Result<Profile, ProfileError> {
    let exceeded = Arc::new(AtomicBool::new(false));
    let compressed = Limited {
        input,
        remaining: compressed,
        exceeded: exceeded.clone(),
    };
    let expanded = Limited {
        // Consume every gzip member and validate the final trailer. A single
        // member decoder can silently leave appended bytes unexamined.
        input: MultiGzDecoder::new(compressed),
        remaining: expanded,
        exceeded: exceeded.clone(),
    };
    let mut collector = Collector {
        profile: Profile {
            phases: vec![],
            lanes: vec![],
        },
        lane_of: HashMap::new(),
        names: HashMap::new(),
        finishing: None,
        event_count: 0,
        event_limit: events,
        budget: JsonBudget {
            remaining: budget.saturating_sub(64),
        },
        incomplete: false,
    };
    // serde_json reads individual bytes. Buffer after the expanded-byte limit
    // so gzip inflation works in chunks while both byte limits remain enforced.
    let mut decoder = serde_json::Deserializer::from_reader(BufReader::new(expanded));
    let result = serde::de::Deserializer::deserialize_map(
        &mut decoder,
        Trace {
            collector: &mut collector,
            found_events: false,
        },
    )
    .and_then(|()| decoder.end());
    if let Err(error) = result {
        return if collector.incomplete || exceeded.load(Ordering::Relaxed) {
            Err(ProfileError::Incomplete(
                "profile byte, event or IPC limit exceeded".to_string(),
            ))
        } else {
            Err(ProfileError::Parse(error.to_string()))
        };
    }
    collector.finish()
}

#[tracing::instrument(target = "zeal::operations", name = "detail.profile_parse", skip_all)]
pub fn parse_bounded<R: Read>(input: R, ipc_budget: usize) -> Result<Profile, ProfileError> {
    parse_limits(
        input,
        COMPRESSED_LIMIT,
        EXPANDED_LIMIT,
        EVENT_LIMIT,
        ipc_budget,
    )
}

#[cfg(test)]
pub fn parse<R: Read>(input: R) -> Result<Profile, String> {
    parse_bounded(input, IPC_LIMIT).map_err(|error| match error {
        ProfileError::Incomplete(note) | ProfileError::Parse(note) => note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use serde_json::{Value, json};
    use std::fs::File;

    fn fixture_path(case: &str) -> String {
        format!(
            "{}/tests/fixtures/run/{case}/profile.gz",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    fn fixture(case: &str) -> Profile {
        parse(File::open(fixture_path(case)).unwrap()).unwrap()
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
        parse(gz.finish().unwrap().as_slice())
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
        assert!(parse(cut).is_err());
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
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        std::io::Write::write_all(&mut gz, bytes).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn byte_event_and_ipc_limits_are_incomplete() {
        let input = gzip(br#"{"traceEvents":[{"name":"Finishing","ph":"X","ts":10}]}"#);
        assert!(matches!(
            parse_limits(input.as_slice(), input.len() - 1, 1024, 10, 1024),
            Err(ProfileError::Incomplete(_))
        ));
        assert!(matches!(
            parse_limits(input.as_slice(), 1024, 20, 10, 1024),
            Err(ProfileError::Incomplete(_))
        ));
        assert!(matches!(
            parse_limits(input.as_slice(), 1024, 1024, 0, 1024),
            Err(ProfileError::Incomplete(_))
        ));
        assert!(matches!(
            parse_limits(input.as_slice(), 1024, 1024, 10, 90),
            Err(ProfileError::Incomplete(_))
        ));
        assert!(parse_limits(input.as_slice(), input.len(), 1024, 1, 1024).is_ok());
    }

    #[test]
    fn metadata_and_unknown_events_count_against_event_limit() {
        let input =
            gzip(br#"{"traceEvents":[{"ph":"M"},{"unknown":true},{"name":"Finishing","ts":10}]}"#);
        assert!(matches!(
            parse_limits(input.as_slice(), 1024, 1024, 2, 1024),
            Err(ProfileError::Incomplete(_))
        ));
    }

    #[test]
    fn malformed_gzip_or_json_is_parse_error() {
        assert!(matches!(
            parse_bounded(b"broken".as_slice(), IPC_LIMIT),
            Err(ProfileError::Parse(_))
        ));
        assert!(matches!(
            parse_bounded(gzip(br#"{"traceEvents":["#).as_slice(), IPC_LIMIT),
            Err(ProfileError::Parse(_))
        ));
    }

    #[test]
    fn appended_garbage_and_second_json_are_parse_errors() {
        let valid = gzip(br#"{"traceEvents":[{"name":"Finishing","ts":1}]}"#);
        let mut garbage = valid.clone();
        garbage.extend_from_slice(b"trailing garbage");
        assert!(matches!(
            parse_bounded(garbage.as_slice(), IPC_LIMIT),
            Err(ProfileError::Parse(_))
        ));

        let mut second_json = valid.clone();
        second_json.extend(gzip(br#"{"traceEvents":[]}"#));
        assert!(matches!(
            parse_bounded(second_json.as_slice(), IPC_LIMIT),
            Err(ProfileError::Parse(_))
        ));

        let mut broken_member = valid;
        let whitespace = gzip(b" ");
        broken_member.extend_from_slice(&whitespace[..whitespace.len() - 1]);
        assert!(matches!(
            parse_bounded(broken_member.as_slice(), IPC_LIMIT),
            Err(ProfileError::Parse(_))
        ));
    }

    #[test]
    fn concatenated_members_share_compressed_and_expanded_limits() {
        // A gzip stream can split one JSON document across members. Every
        // member still passes through the same byte counters.
        let mut input = gzip(br#"{"traceEvents":["#);
        input.extend(gzip(br#"{"name":"Finishing","ts":1}]}"#));
        assert!(parse_limits(input.as_slice(), input.len(), 1024, 1, 1024).is_ok());
        assert!(matches!(
            parse_limits(input.as_slice(), input.len() - 1, 1024, 1, 1024),
            Err(ProfileError::Incomplete(_))
        ));
        assert!(matches!(
            parse_limits(input.as_slice(), input.len(), 25, 1, 1024),
            Err(ProfileError::Incomplete(_))
        ));
    }

    /// Generated inputs keep fixtures small. Run directly under `/usr/bin/time -l`
    /// to record actual process RSS separately from compressed/expanded bytes.
    #[test]
    #[ignore = "manual near-limit profile measurement"]
    fn measure_profile_limits() {
        // The fixed JSON wrapper consumes 71 bytes. Leave a small margin.
        measure("expanded_near_64MiB", EXPANDED_LIMIT - 128, 1, false, false);
        measure("expanded_over_64MiB", EXPANDED_LIMIT, 1, false, true);
        measure("events_at_250k", 0, EVENT_LIMIT, false, false);
        measure("events_over_250k", 0, EVENT_LIMIT + 1, false, true);
        measure("event_over_IPC32MiB", 0, 1, true, true);
    }

    fn write_chunks(write: &mut impl FnMut(&[u8]), chunk: &[u8], count: usize) {
        for _ in 0..count {
            write(chunk);
        }
    }

    fn measure(
        name: &str,
        padding: usize,
        event_count: usize,
        heavy_event: bool,
        incomplete: bool,
    ) {
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        let mut expanded = 0usize;
        let mut write = |bytes: &[u8]| {
            std::io::Write::write_all(&mut gz, bytes).unwrap();
            expanded += bytes.len();
        };
        write(br#"{"padding":""#);
        let chunk = [b'x'; 8192];
        write_chunks(&mut write, &chunk, padding / chunk.len());
        write(&chunk[..padding % chunk.len()]);
        write(br#"","traceEvents":[{"name":"Finishing","ts":1}"#);
        for _ in 1..event_count {
            write(br#",{"name":"event","tid":1}"#);
        }
        if heavy_event {
            write(br#",{"name":""#);
            write_chunks(&mut write, &chunk, IPC_LIMIT / chunk.len() + 1);
            write(br#""}"#);
        }
        write(b"]}");
        let compressed = gz.finish().unwrap();
        let began = std::time::Instant::now();
        let result = parse_bounded(compressed.as_slice(), IPC_LIMIT);
        let milliseconds = began.elapsed().as_secs_f64() * 1000.0;
        let ipc = if let Ok(profile) = &result {
            let mut budget = JsonBudget {
                remaining: usize::MAX,
            };
            budget.charge(profile).unwrap();
            usize::MAX - budget.remaining
        } else {
            0
        };
        eprintln!(
            "profile_measure name={name} expanded_bytes={expanded} compressed_bytes={} input_events={} result_ipc_bytes={ipc} parse_ms={milliseconds:.3} status={}",
            compressed.len(),
            event_count + usize::from(heavy_event),
            if result.is_ok() { "ok" } else { "incomplete" }
        );
        if incomplete {
            assert!(matches!(result, Err(ProfileError::Incomplete(_))));
        } else {
            assert!(result.is_ok());
        }
    }
}
