//! Read a Bazel JSON trace profile (`--profile=profile.gz`) into what the
//! timing view draws.
//!
//! - **phases**: `build phase marker` events in `ts` order. Each phase runs
//!   until the next marker; the last one runs until the `Finishing` event.
//!   Names are kept as Bazel writes them (under Skymeld, loading / analysis /
//!   execution share one marker).
//! - **lanes**: threads named `skyframe-evaluator-execution-*`, numbered in
//!   the order their `thread_name` metadata appears (shown as `exec-0`, ...).
//!   Only their count is kept.
//! - **bars**: `action processing` events on those lanes.

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub phases: Vec<Phase>,
    pub lane_count: usize,
    pub bars: Vec<Bar>,
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
pub struct Bar {
    /// Lane number, `0..Profile::lane_count`.
    pub lane: usize,
    pub start_us: i64,
    pub dur_us: i64,
    pub name: String,
    pub mnemonic: String,
}

#[derive(Deserialize)]
struct Trace {
    #[serde(rename = "traceEvents")]
    trace_events: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    #[serde(default)]
    cat: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    ph: String,
    #[serde(default)]
    ts: i64,
    #[serde(default)]
    dur: i64,
    #[serde(default)]
    tid: u64,
    #[serde(default)]
    args: Args,
}

#[derive(Default, Deserialize)]
struct Args {
    /// `thread_name` metadata: the thread's name.
    #[serde(default)]
    name: String,
    /// `action processing`: the action's mnemonic.
    #[serde(default)]
    mnemonic: String,
}

/// Parse a gzipped profile. A file that is not complete gzip + JSON is `Err`.
pub fn parse<R: Read>(input: R) -> Result<Profile, String> {
    let trace: Trace = serde_json::from_reader(GzDecoder::new(input)).map_err(|e| e.to_string())?;
    let events = trace.trace_events;

    let mut markers: Vec<&Event> = events
        .iter()
        .filter(|e| e.cat == "build phase marker")
        .collect();
    markers.sort_by_key(|e| e.ts);
    let finishing = events
        .iter()
        .find(|e| e.name == "Finishing")
        .ok_or("no Finishing event")?
        .ts;
    let ends = markers.iter().skip(1).map(|e| e.ts).chain([finishing]);
    let phases = markers
        .iter()
        .zip(ends)
        .map(|(e, end_us)| Phase {
            name: e.name.clone(),
            start_us: e.ts,
            end_us,
        })
        .collect();

    let mut lane_of: HashMap<u64, usize> = HashMap::new();
    for e in &events {
        if e.ph == "M"
            && e.name == "thread_name"
            && e.args.name.starts_with("skyframe-evaluator-execution-")
        {
            let next = lane_of.len();
            lane_of.entry(e.tid).or_insert(next);
        }
    }
    let bars = events
        .iter()
        .filter(|e| e.cat == "action processing")
        .filter_map(|e| {
            Some(Bar {
                lane: *lane_of.get(&e.tid)?,
                start_us: e.ts,
                dur_us: e.dur,
                name: e.name.clone(),
                mnemonic: e.args.mnemonic.clone(),
            })
        })
        .collect();

    Ok(Profile {
        phases,
        lane_count: lane_of.len(),
        bars,
    })
}
