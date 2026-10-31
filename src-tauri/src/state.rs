//! Shared application state held by the Rust core.

use crate::snapshot::Snapshot;
use serde::Serialize;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use tauri::{AppHandle, Emitter};

/// Serializable copy of everything the UI needs, pushed as the `app-state`
/// event on every change.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStateDto {
    pub bes_error: Option<String>,
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
    /// The Console whose output can still be read in the open workspace.
    pub console_run: Option<RunId>,
    /// A stop was accepted for `running`; it stays until the run leaves the
    /// slot, however it ends.
    pub stopping: bool,
    /// Why the last run's exit is unknown: its `wait` failed, or its group was
    /// killed and no exit was confirmed.
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

/// Identifier for one run owned by the application.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    /// Wraps a run identifier from a persisted run record.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Returns the persisted identifier text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for RunId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for RunId {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned())
    }
}

impl Deref for RunId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

/// Read access to the last Console. This outlives process control and is never
/// used to classify or persist invocation history.
pub struct ConsoleSession {
    pub id: RunId,
    pub workspace: PathBuf,
    /// The capture worker has stopped writing, including its final drain.
    pub finished: bool,
}

pub struct CoreState {
    pub bes_error: Option<String>,
    /// Only this process's bound receiver may be used for a new Bazel child.
    pub bes_endpoint: Option<String>,
    pub workspace: Option<PathBuf>,
    pub bazel: Option<PathBuf>,
    /// The latest fully built snapshot; swapped atomically per fetch.
    pub snapshot: Option<Arc<Snapshot>>,
    pub last_error: Option<IngestError>,
    /// At most one query or run is active. Busy requests are rejected.
    pub ingest_running: bool,
    /// The run in flight.
    pub run: Option<RunId>,
    pub console: Option<ConsoleSession>,
    /// Process group of the run in flight (the child's pid), for the one stop
    /// that signals it.
    pub run_terminal: Option<crate::terminal::Control>,
    pub run_pgid: Option<i32>,
    /// A stop was accepted for `run`: its group was signalled and a repeated
    /// stop does nothing.
    pub run_stopping: bool,
    /// First stop request; the child owner applies its escalation deadlines.
    pub run_stop_started: Option<std::time::Instant>,
    /// Message for the UI about why the last run's exit is unknown.
    pub run_note: Option<String>,
}

impl CoreState {
    pub fn console_run(&self) -> Option<&RunId> {
        self.console
            .as_ref()
            .filter(|console| self.workspace.as_ref() == Some(&console.workspace))
            .map(|console| &console.id)
    }
}

pub struct Core {
    pub state: Mutex<CoreState>,
    pub run_finished: Condvar,
}

impl Core {
    #[must_use]
    pub fn new() -> Self {
        Core {
            state: Mutex::new(CoreState {
                bes_error: None,
                bes_endpoint: None,
                workspace: None,
                bazel: None,
                snapshot: None,
                last_error: None,
                ingest_running: false,
                run: None,
                console: None,
                run_pgid: None,
                run_terminal: None,
                run_stopping: false,
                run_stop_started: None,
                run_note: None,
            }),
            run_finished: Condvar::new(),
        }
    }

    #[must_use]
    pub fn dto(&self) -> AppStateDto {
        let st = self.state.lock().expect("application state lock poisoned");
        AppStateDto {
            bes_error: st.bes_error.clone(),
            workspace: st
                .workspace
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            bazel: st.bazel.as_ref().map(|p| p.to_string_lossy().into_owned()),
            ingest: if st.ingest_running { "running" } else { "idle" },
            snapshot: st.snapshot.clone(),
            last_error: st.last_error.clone(),
            running: st.run.clone(),
            console_run: st.console_run().cloned(),
            stopping: st.run_stopping,
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

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dto_reports_the_stopping_run_and_its_note() {
        let core = Core::new();
        let idle = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(idle["running"], serde_json::Value::Null);
        assert_eq!(idle["stopping"], false);
        assert_eq!(idle["runNote"], serde_json::Value::Null);

        {
            let mut st = core.state.lock().unwrap();
            st.run = Some("abc".into());
            st.run_stopping = true;
            st.run_note = Some("note".to_string());
        }
        let json = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(json["running"], "abc");
        assert_eq!(json["stopping"], true);
        assert_eq!(json["runNote"], "note");
    }

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
