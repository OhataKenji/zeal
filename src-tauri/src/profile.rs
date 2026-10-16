//! Read a Bazel JSON trace profile (`--profile=profile.gz`) into what the
//! timing view draws.
//!
//! - **phases**: `build phase marker` events in `ts` order. Each phase runs
//!   until the next marker; the last one runs until the `Finishing` event.
//!   Names are kept as Bazel writes them (under Skymeld, loading / analysis /
//!   execution share one marker).
//! - **lanes**: one per thread (`tid`), in the order the thread's first
//!   `ph: "X"` event appears. All such events on the thread are its `spans`
//!   in file order — the timing view draws every thread, not only execution
//!   lanes. `thread_name` metadata supplies the label; it may arrive after
//!   the thread's events.

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
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
    pub spans: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub start_us: i64,
    pub dur_us: i64,
    /// `ProfilerTask` category, e.g. `action processing`.
    pub cat: String,
    pub name: String,
    /// `args.mnemonic`; empty when the event doesn't carry one.
    pub mnemonic: String,
    /// Primary output path when Bazel recorded one on the event.
    pub out: Option<String>,
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
    out: Option<String>,
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
    let mut lanes: Vec<Lane> = Vec::new();
    for e in &events {
        if e.ph != "X" {
            continue;
        }
        let i = *lane_of.entry(e.tid).or_insert_with(|| {
            lanes.push(Lane {
                name: String::new(),
                tid: e.tid,
                spans: Vec::new(),
            });
            lanes.len() - 1
        });
        lanes[i].spans.push(Span {
            start_us: e.ts,
            dur_us: e.dur,
            cat: e.cat.clone(),
            name: e.name.clone(),
            mnemonic: e.args.mnemonic.clone(),
            out: e.out.clone(),
        });
    }
    for e in &events {
        if e.ph == "M"
            && e.name == "thread_name"
            && let Some(&i) = lane_of.get(&e.tid)
        {
            lanes[i].name.clone_from(&e.args.name);
        }
    }

    Ok(Profile { phases, lanes })
}
