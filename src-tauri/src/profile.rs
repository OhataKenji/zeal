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
mod tests;
