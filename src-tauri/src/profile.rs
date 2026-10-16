//! Read a Bazel JSON trace profile (`--profile=profile.gz`) into what the
//! timing view draws.
//!
//! - **phases**: `build phase marker` events in `ts` order. Each phase runs
//!   until the next marker; the last one runs until the `Finishing` event.
//!   Names are kept as Bazel writes them (under Skymeld, loading / analysis /
//!   execution share one marker).
//! - **lanes**: one per thread (`tid`), in the order the thread's first event
//!   appears. Events pass through verbatim, in file order — the only
//!   processing is grouping by `tid` and folding `thread_name` (`ph: "M"`)
//!   metadata into the lane's `name`.

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;

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
    /// `thread_name` metadata; empty when the thread never got one.
    pub name: String,
    pub tid: u64,
    /// The thread's events, verbatim, in file order.
    pub events: Vec<Value>,
}

#[derive(Deserialize)]
struct Trace {
    #[serde(rename = "traceEvents")]
    trace_events: Vec<Value>,
}

fn str_of<'a>(e: &'a Value, key: &str) -> Option<&'a str> {
    e.get(key).and_then(Value::as_str)
}

/// Parse a gzipped profile. A file that is not complete gzip + JSON is `Err`.
pub fn parse<R: Read>(input: R) -> Result<Profile, String> {
    let trace: Trace = serde_json::from_reader(GzDecoder::new(input)).map_err(|e| e.to_string())?;
    let events = trace.trace_events;

    let mut markers: Vec<&Value> = events
        .iter()
        .filter(|e| str_of(e, "cat") == Some("build phase marker"))
        .collect();
    markers.sort_by_key(|e| e["ts"].as_i64().unwrap_or(0));
    let finishing = events
        .iter()
        .find(|e| str_of(e, "name") == Some("Finishing"))
        .and_then(|e| e["ts"].as_i64())
        .ok_or("no Finishing event")?;
    let ends = markers
        .iter()
        .map(|e| e["ts"].as_i64().unwrap_or(0))
        .skip(1)
        .chain([finishing]);
    let phases = markers
        .iter()
        .zip(ends)
        .map(|(e, end_us)| Phase {
            name: str_of(e, "name").unwrap_or_default().to_string(),
            start_us: e["ts"].as_i64().unwrap_or(0),
            end_us,
        })
        .collect();

    let mut lane_of: HashMap<u64, usize> = HashMap::new();
    let mut lanes: Vec<Lane> = Vec::new();
    for e in &events {
        if str_of(e, "ph") == Some("M") {
            continue;
        }
        let tid = e["tid"].as_u64().unwrap_or(0);
        let i = *lane_of.entry(tid).or_insert_with(|| {
            lanes.push(Lane {
                name: String::new(),
                tid,
                events: Vec::new(),
            });
            lanes.len() - 1
        });
        lanes[i].events.push(e.clone());
    }
    for e in &events {
        if str_of(e, "ph") == Some("M")
            && str_of(e, "name") == Some("thread_name")
            && let Some(name) = e["args"]["name"].as_str()
            && let Some(&i) = lane_of.get(&e["tid"].as_u64().unwrap_or(0))
        {
            lanes[i].name = name.to_string();
        }
    }

    Ok(Profile { phases, lanes })
}
