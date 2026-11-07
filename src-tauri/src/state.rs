//! Shared application state held by the Rust core.

use crate::snapshot::TargetIndex;
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
    /// Whether the receiver can accept a new Bazel invocation.
    pub bes_ready: bool,
    /// Workspace root path, if one is open.
    pub workspace: Option<String>,
    pub scope: crate::scope::Scope,
    /// Resolved bazel executable, if found on PATH.
    pub bazel: Option<String>,
    /// "idle" | "running": whether a query ingestion is in flight.
    pub ingest: &'static str,
    /// `fetched_at` of the current snapshot, kept across failed refreshes. The
    /// UI fetches the target index through `get_snapshot` only when this
    /// changes, so events never carry target data.
    pub snapshot_fetched_at: Option<u64>,
    /// The most recent fetch failure (or partial-fetch cause). Cleared by a
    /// fully successful fetch.
    pub last_error: Option<IngestError>,
    /// Id of the Bazel operation in flight, if any.
    pub active_invocation: Option<InvocationId>,
    /// The Console whose output can still be read in the open workspace.
    pub console_invocation: Option<InvocationId>,
    /// A stop was accepted for `active_invocation`; it stays until the
    /// Bazel operation leaves the slot, however it ends.
    pub stopping: bool,
    /// Why the last Bazel process's exit is unknown: its `wait` failed, or its
    /// group was killed and no exit was confirmed.
    pub invocation_note: Option<String>,
    /// "idle" | "running": whether a selected target's detail is loading.
    pub detail_query: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestError {
    /// Exit code of the query process, if it ran to an exit.
    pub exit_code: Option<i32>,
    /// Short human-readable summary (tail of stderr, timeout, spawn error).
    pub summary: String,
}

/// Identifier for one invocation owned by the application.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct InvocationId(String);

impl InvocationId {
    /// Identifier for an invocation record or active Bazel operation.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Returns the persisted identifier text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for InvocationId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for InvocationId {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned())
    }
}

impl Deref for InvocationId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

/// The Bazel operation holding the slot. Its fields exist exactly while it
/// owns the slot: releasing the slot drops them together,
/// so a group id can never outlive the operation it belongs to. `pgid` is
/// revoked at reap so a racing stop can never signal a recycled process group.
pub struct ActiveInvocation {
    /// Identifier for the Bazel operation in flight.
    pub id: InvocationId,
    /// Present for terminal output; queries use stdout/stderr pipes.
    pub terminal: Option<crate::terminal::Control>,
    /// The child's process group, for the one stop that signals it; `None`
    /// once the owner has reaped the child.
    pub pgid: Option<i32>,
    /// A stop was accepted for `id`: its group was signalled and a repeated
    /// stop does nothing. It stays until the operation leaves the slot,
    /// however it ends.
    pub stopping: bool,
    /// First stop request; the child owner applies its escalation deadlines.
    pub stop_started: Option<std::time::Instant>,
}

impl ActiveInvocation {
    /// A held slot for state tests; its terminal is detached from any pty.
    #[cfg(test)]
    pub fn stub(id: &str, pgid: Option<i32>) -> Self {
        Self {
            id: id.into(),
            terminal: Some(crate::terminal::Control::stub()),
            pgid,
            stopping: false,
            stop_started: None,
        }
    }
}

/// Read access to the last Console. This outlives process control and is never
/// used to classify or persist invocation history.
pub struct ConsoleSession {
    pub id: InvocationId,
    pub workspace: PathBuf,
    /// The session's captured terminal output; an already-unlinked temp file.
    pub log: std::fs::File,
    /// The capture worker has stopped writing, including its final drain.
    pub finished: bool,
}

pub struct CoreState {
    pub bes_error: Option<String>,
    /// Only this process's bound receiver may be used for a new Bazel child.
    pub bes_endpoint: Option<String>,
    pub workspace: Option<PathBuf>,
    pub scope: crate::scope::Scope,
    pub bazel: Option<PathBuf>,
    /// The latest target index; swapped atomically per fetch.
    pub snapshot: Option<Arc<TargetIndex>>,
    pub last_error: Option<IngestError>,
    /// The index/detail phase flags and process slot serialize Bazel work.
    /// Busy requests are rejected.
    pub ingest_running: bool,
    /// The Bazel operation holding the slot; `None` when no process is in
    /// flight.
    pub active_invocation: Option<ActiveInvocation>,
    pub console: Option<ConsoleSession>,
    /// Message for the UI about why the last process exit is unknown.
    pub invocation_note: Option<String>,
    /// Scoped proto query for a selected target.
    pub detail_query_running: bool,
}

impl CoreState {
    /// Whether Bazel work or an index/detail phase occupies the workspace.
    pub fn bazel_busy(&self) -> bool {
        self.ingest_running || self.active_invocation.is_some() || self.detail_query_running
    }
}

impl CoreState {
    pub fn console_invocation(&self) -> Option<&InvocationId> {
        self.console
            .as_ref()
            .filter(|console| self.workspace.as_ref() == Some(&console.workspace))
            .map(|console| &console.id)
    }
}

pub struct Core {
    pub state: Mutex<CoreState>,
    pub invocation_finished: Condvar,
}

impl Core {
    #[must_use]
    pub fn new() -> Self {
        Core {
            state: Mutex::new(CoreState {
                bes_error: None,
                bes_endpoint: None,
                workspace: None,
                scope: crate::scope::Scope::default(),
                bazel: None,
                snapshot: None,
                last_error: None,
                ingest_running: false,
                active_invocation: None,
                console: None,
                invocation_note: None,
                detail_query_running: false,
            }),
            invocation_finished: Condvar::new(),
        }
    }

    #[must_use]
    pub fn dto(&self) -> AppStateDto {
        let st = self.state.lock().expect("application state lock poisoned");
        AppStateDto {
            bes_error: st.bes_error.clone(),
            bes_ready: st.bes_endpoint.is_some(),
            scope: st.scope.clone(),
            workspace: st
                .workspace
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            bazel: st.bazel.as_ref().map(|p| p.to_string_lossy().into_owned()),
            ingest: if st.ingest_running { "running" } else { "idle" },
            snapshot_fetched_at: st
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.meta.fetched_at),
            last_error: st.last_error.clone(),
            active_invocation: st
                .active_invocation
                .as_ref()
                .map(|invocation| invocation.id.clone()),
            console_invocation: st.console_invocation().cloned(),
            stopping: st
                .active_invocation
                .as_ref()
                .is_some_and(|invocation| invocation.stopping),
            invocation_note: st.invocation_note.clone(),
            detail_query: if st.detail_query_running {
                "running"
            } else {
                "idle"
            },
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
    fn dto_reports_bes_readiness_independently_of_its_message() {
        let core = Core::new();
        assert_eq!(serde_json::to_value(core.dto()).unwrap()["besReady"], false);

        {
            let mut st = core.state.lock().unwrap();
            st.bes_endpoint = Some("grpc://127.0.0.1:12345".to_string());
            st.bes_error = Some("preferred port occupied".to_string());
        }
        let available = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(available["besReady"], true);
        assert_eq!(available["besError"], "preferred port occupied");

        {
            let mut st = core.state.lock().unwrap();
            st.bes_endpoint = None;
            st.bes_error = Some("receiver stopped".to_string());
        }
        let unavailable = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(unavailable["besReady"], false);
        assert_eq!(unavailable["besError"], "receiver stopped");
    }

    #[test]
    fn dto_reports_the_stopping_invocation_and_its_note() {
        let core = Core::new();
        let idle = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(idle["activeInvocation"], serde_json::Value::Null);
        assert_eq!(idle["stopping"], false);
        assert_eq!(idle["invocationNote"], serde_json::Value::Null);

        {
            let mut st = core.state.lock().unwrap();
            st.active_invocation = Some(ActiveInvocation {
                stopping: true,
                ..ActiveInvocation::stub("abc", Some(1))
            });
            st.invocation_note = Some("note".to_string());
        }
        let json = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(json["activeInvocation"], "abc");
        assert_eq!(json["stopping"], true);
        assert_eq!(json["invocationNote"], "note");
    }

    /// Every way the serialized slot can be held makes `bazel_busy` true.
    #[test]
    fn bazel_busy_covers_ingest_invocation_and_detail_query() {
        let core = Core::new();
        assert!(!core.state.lock().unwrap().bazel_busy());
        {
            let mut st = core.state.lock().unwrap();
            st.ingest_running = true;
            assert!(st.bazel_busy());
            st.ingest_running = false;
            st.active_invocation = Some(ActiveInvocation::stub("r", None));
            assert!(st.bazel_busy());
            st.active_invocation = None;
            st.detail_query_running = true;
            assert!(st.bazel_busy());
        }
    }

    #[test]
    fn dto_reports_the_detail_query_state() {
        let core = Core::new();
        let idle = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(idle["detailQuery"], "idle");

        {
            let mut st = core.state.lock().unwrap();
            st.detail_query_running = true;
        }
        let json = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(json["detailQuery"], "running");
    }

    #[test]
    fn the_dto_names_the_snapshot_without_carrying_its_data() {
        use crate::snapshot::{Meta, Status};

        let snapshot = TargetIndex {
            meta: Meta {
                fetched_at: 1,
                status: Status::Partial,
            },
            targets: Vec::new(),
        };
        let core = Core::new();
        core.state.lock().unwrap().snapshot = Some(Arc::new(snapshot));

        let json = serde_json::to_value(core.dto()).unwrap();
        assert_eq!(
            json["snapshotFetchedAt"],
            serde_json::json!(1),
            "the event names the snapshot; its data goes through get_snapshot"
        );
        assert!(json.get("snapshot").is_none());
    }
}
