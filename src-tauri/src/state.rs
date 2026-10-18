//! Shared application state held by the Rust core.

use crate::snapshot::Snapshot;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

/// Serializable copy of everything the UI needs, pushed as the `app-state`
/// event on every change.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStateDto {
    /// Workspace root path, if one is open.
    pub workspace: Option<String>,
    /// Resolved bazel executable, if found on PATH.
    pub bazel: Option<String>,
    /// "idle" | "running": whether a query ingestion is in flight.
    pub ingest: &'static str,
    /// The current snapshot. Kept across failed refreshes. The DTO carries the
    /// `Arc` so that an emit clones a pointer, not the whole snapshot.
    pub snapshot: Option<Arc<Snapshot>>,
    /// The most recent fetch failure (or partial-fetch cause). Cleared by a
    /// fully successful fetch.
    pub last_error: Option<IngestError>,
    /// Id of the run in flight, if any.
    pub running: Option<RunId>,
    /// Why the last run's exit or saved result is unknown.
    pub run_note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestError {
    /// Exit code of the query process, if it ran to an exit.
    pub exit_code: Option<i32>,
    /// Short human-readable summary (tail of stderr, timeout, spawn error).
    pub summary: String,
}

pub type RunId = String;

pub struct CoreState {
    pub workspace: Option<PathBuf>,
    pub bazel: Option<PathBuf>,
    /// The latest fully built snapshot; swapped atomically per fetch.
    pub snapshot: Option<Arc<Snapshot>>,
    pub last_error: Option<IngestError>,
    /// At most one query runs; busy requests are coalesced into one follow-up.
    pub ingest_running: bool,
    pub ingest_pending: bool,
    /// The run in flight.
    pub run: Option<RunId>,
    /// Message for the UI about why the last run's exit is unknown.
    pub run_note: Option<String>,
}

pub struct Core {
    pub state: Mutex<CoreState>,
}

impl Core {
    pub fn new() -> Self {
        Core {
            state: Mutex::new(CoreState {
                workspace: None,
                bazel: None,
                snapshot: None,
                last_error: None,
                ingest_running: false,
                ingest_pending: false,
                run: None,
                run_note: None,
            }),
        }
    }

    pub fn dto(&self) -> AppStateDto {
        let st = self.state.lock().unwrap();
        AppStateDto {
            workspace: st
                .workspace
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            bazel: st.bazel.as_ref().map(|p| p.to_string_lossy().into_owned()),
            ingest: if st.ingest_running { "running" } else { "idle" },
            snapshot: st.snapshot.clone(),
            last_error: st.last_error.clone(),
            running: st.run.clone(),
            run_note: st.run_note.clone(),
        }
    }

    /// Push the current state to the webview.
    pub fn emit(&self, app: &AppHandle) {
        if let Err(e) = app.emit("app-state", self.dto()) {
            log::warn!("emit app-state failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dto_serializes_the_snapshot_unchanged() {
        use crate::snapshot::{Meta, Status};

        let snapshot = Snapshot {
            meta: Meta {
                fetched_at: 1,
                status: Status::Ok,
            },
            targets: Vec::new(),
            edges: Vec::new(),
            files: Vec::new(),
        };
        let expected = serde_json::to_value(&snapshot).unwrap();
        let core = Core::new();
        core.state.lock().unwrap().snapshot = Some(Arc::new(snapshot));

        let json = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(
            json["snapshot"], expected,
            "the Arc must not change the JSON"
        );
    }
}
