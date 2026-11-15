//! Shared application state held by the Rust core.
//!
//! One mutex guards workspace and owned-invocation maps plus the shutdown
//! flag; it serializes map ownership and process reap/signal decisions, not
//! admission capacity. Every Bazel operation Zeal starts gets its own
//! [`OwnedInvocation`] entry keyed by a fresh id, so a later launch never
//! overwrites an earlier one. External BES invocations are only observed and
//! are never inserted here.

use crate::snapshot::TargetIndex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// Serializable copy of everything the UI needs, pushed as the `app-state`
/// event on every change. `revision` grows on every snapshot so out-of-order
/// pushes cannot overwrite newer state.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStateDto {
    pub revision: u64,
    pub bes_error: Option<String>,
    /// Whether the receiver can accept a new Bazel invocation.
    pub bes_ready: bool,
    /// Resolved bazel executable, if found on PATH.
    pub bazel: Option<String>,
    /// Saved preference for the last opened workspace; never used to resolve
    /// an operation, which always names its workspace explicitly.
    pub last_workspace: Option<String>,
    pub workspaces: Vec<WorkspaceDto>,
    pub invocations: Vec<OwnedInvocationDto>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDto {
    pub workspace: String,
    pub scope: crate::scope::Scope,
    /// "idle" | "running": whether a target-list fetch is in flight.
    pub ingest: &'static str,
    /// `fetched_at` of the current snapshot, kept across failed refreshes. The
    /// UI fetches the target index through `get_snapshot` only when this
    /// changes, so events never carry target data.
    pub snapshot_fetched_at: Option<u64>,
    /// The most recent fetch failure (or partial-fetch cause). Cleared by a
    /// fully successful fetch.
    pub last_error: Option<IngestError>,
}

/// One Bazel operation Zeal launched: build/test/run under a terminal, or a
/// query/cquery/aquery over pipes. It describes the local process only and is
/// never persisted as BEP history.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedInvocationDto {
    pub id: String,
    pub workspace: String,
    pub cmd: String,
    pub pattern: String,
    /// Milliseconds since the Unix epoch, the store's timestamp convention.
    pub started_at: i64,
    /// A process is still owned, including the final output drain.
    pub running: bool,
    /// The owned process group may still be signalled.
    pub stoppable: bool,
    /// A stop was accepted; it stays until the process leaves the entry.
    pub stopping: bool,
    /// Retained console output exists for this invocation.
    pub console: bool,
    /// The owned process's exit code, kept apart from any BEP exit code.
    pub process_exit_code: Option<i32>,
    /// Why the process exit is unknown or noteworthy (e.g. lock contention).
    pub note: Option<String>,
}

/// Result of registering a workspace: its canonical path plus the new state.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWorkspaceResult {
    pub workspace: String,
    pub state: AppStateDto,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestError {
    /// Exit code of the query process, if it ran to an exit.
    pub exit_code: Option<i32>,
    /// Short human-readable summary (tail of stderr, timeout, spawn error).
    pub summary: String,
}

/// The Bazel operation holding an invocation entry's process slot. Its fields
/// exist exactly while it owns the process: releasing it drops them together,
/// so a group id can never outlive the operation it belongs to. `pgid` is
/// revoked at reap so a racing stop can never signal a recycled process group.
pub struct ActiveInvocation {
    /// Present for terminal output; queries use stdout/stderr pipes.
    pub terminal: Option<crate::terminal::Control>,
    /// The child's process group, for the one stop that signals it; `None`
    /// once the owner has reaped the child.
    pub pgid: Option<i32>,
    /// First stop request: its group was signalled and a repeated stop does
    /// nothing. It stays until the process leaves the entry, however it ends,
    /// and the child owner applies its escalation deadlines from it.
    pub stop_started: Option<std::time::Instant>,
}

impl ActiveInvocation {
    /// A held process for state tests; its terminal is detached from any pty.
    #[cfg(test)]
    pub fn stub(pgid: Option<i32>) -> Self {
        Self {
            terminal: Some(crate::terminal::Control::stub()),
            pgid,
            stop_started: None,
        }
    }
}

/// Target discovery state for one registered workspace. It owns no execution
/// slot: Bazel operations live in [`CoreState::invocations`] instead.
pub struct WorkspaceState {
    pub scope: crate::scope::Scope,
    /// The latest target index; swapped atomically per fetch.
    pub snapshot: Option<Arc<TargetIndex>>,
    pub last_error: Option<IngestError>,
    /// Token of the in-flight ingest request, if any.
    pub ingest_request: Option<String>,
}

impl WorkspaceState {
    pub fn new(scope: crate::scope::Scope) -> Self {
        Self {
            scope,
            snapshot: None,
            last_error: None,
            ingest_request: None,
        }
    }
}

/// One locally launched Bazel operation. A console keeps its entry for the
/// app's lifetime so finished consoles stay readable; a console-less entry is
/// released when its worker finishes.
pub struct OwnedInvocation {
    pub workspace: PathBuf,
    pub cmd: String,
    pub pattern: String,
    pub started_at: i64,
    /// `None` once the worker released the slot after its final drain.
    pub process: Option<ActiveInvocation>,
    /// The session's captured terminal output; an already-unlinked temp file.
    /// This outlives the process slot, which is released only after the
    /// capture's final drain.
    pub console: Option<std::fs::File>,
    /// The owned process's exit code, separate from any BEP exit code.
    pub process_exit_code: Option<i32>,
    /// Message for the UI about an unknown or noteworthy process exit.
    pub note: Option<String>,
}

pub struct CoreState {
    pub bes_error: Option<String>,
    /// Only this process's bound receiver may be used for a new Bazel child.
    pub bes_endpoint: Option<String>,
    pub bazel: Option<PathBuf>,
    /// Saved preference only; operations always name their workspace.
    pub last_workspace: Option<PathBuf>,
    /// Registered workspaces by canonical path.
    pub workspaces: BTreeMap<PathBuf, WorkspaceState>,
    /// Every Bazel operation Zeal launched, by invocation id.
    pub invocations: BTreeMap<String, OwnedInvocation>,
    /// Set at app exit: new starts are rejected while owned children stop.
    pub shutting_down: bool,
    revision: u64,
}

impl CoreState {
    /// `ws` is a canonical workspace id returned by `open_workspace`; only an
    /// exact registered key resolves, so a removed directory stays reachable.
    pub fn workspace_path(&self, ws: &str) -> Result<PathBuf, String> {
        self.workspaces
            .get_key_value(Path::new(ws))
            .map(|(path, _)| path.clone())
            .ok_or_else(|| "workspace is not open".to_string())
    }

    /// Register `ws` (already canonicalized) under `scope`, or apply a scope
    /// change to the existing entry: a different scope invalidates the cached
    /// snapshot, error and any in-flight fetch without touching executions.
    pub fn register_workspace(&mut self, ws: PathBuf, scope: crate::scope::Scope) {
        match self.workspaces.get_mut(&ws) {
            Some(state) if state.scope != scope => {
                state.scope = scope;
                state.snapshot = None;
                state.last_error = None;
                state.ingest_request = None;
            }
            Some(_) => {}
            None => {
                self.workspaces.insert(ws, WorkspaceState::new(scope));
            }
        }
    }
}

/// Milliseconds since the Unix epoch, the store's timestamp convention.
pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// The DTO for one owned invocation, shared by `Core::dto` and `runner::start`.
pub(crate) fn owned_dto(id: &str, invocation: &OwnedInvocation) -> OwnedInvocationDto {
    OwnedInvocationDto {
        id: id.to_string(),
        workspace: invocation.workspace.to_string_lossy().into_owned(),
        cmd: invocation.cmd.clone(),
        pattern: invocation.pattern.clone(),
        started_at: invocation.started_at,
        running: invocation.process.is_some(),
        stoppable: invocation
            .process
            .as_ref()
            .is_some_and(|process| process.pgid.is_some()),
        stopping: invocation
            .process
            .as_ref()
            .is_some_and(|process| process.stop_started.is_some()),
        console: invocation.console.is_some(),
        process_exit_code: invocation.process_exit_code,
        note: invocation.note.clone(),
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
                bazel: crate::workspace::resolve_bazel(),
                last_workspace: None,
                workspaces: BTreeMap::new(),
                invocations: BTreeMap::new(),
                shutting_down: false,
                revision: 0,
            }),
            invocation_finished: Condvar::new(),
        }
    }

    /// Snapshot the state for the UI. The revision is bumped while holding
    /// the lock so the emitted order matches the observed mutation order.
    #[must_use]
    pub fn dto(&self) -> AppStateDto {
        let mut st = self.state.lock().expect("application state lock poisoned");
        st.revision += 1;
        let revision = st.revision;
        AppStateDto {
            revision,
            bes_error: st.bes_error.clone(),
            bes_ready: st.bes_endpoint.is_some(),
            bazel: st.bazel.as_ref().map(|p| p.to_string_lossy().into_owned()),
            last_workspace: st
                .last_workspace
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            workspaces: st
                .workspaces
                .iter()
                .map(|(path, state)| WorkspaceDto {
                    workspace: path.to_string_lossy().into_owned(),
                    scope: state.scope.clone(),
                    ingest: if state.ingest_request.is_some() {
                        "running"
                    } else {
                        "idle"
                    },
                    snapshot_fetched_at: state
                        .snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.meta.fetched_at),
                    last_error: state.last_error.clone(),
                })
                .collect(),
            invocations: st
                .invocations
                .iter()
                .map(|(id, invocation)| owned_dto(id, invocation))
                .collect(),
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

    fn owned(ws: &Path) -> OwnedInvocation {
        OwnedInvocation {
            workspace: ws.to_path_buf(),
            cmd: "build".to_string(),
            pattern: "//pkg:b".to_string(),
            started_at: 1,
            process: Some(ActiveInvocation::stub(Some(42))),
            console: None,
            process_exit_code: None,
            note: None,
        }
    }

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
    fn dto_lists_workspaces_and_owned_invocations_with_monotonic_revision() {
        let core = Core::new();
        let ws_a = PathBuf::from("/ws/a");
        let ws_b = PathBuf::from("/ws/b");
        {
            let mut st = core.state.lock().unwrap();
            st.workspaces.insert(
                ws_a.clone(),
                WorkspaceState::new(crate::scope::Scope::default()),
            );
            st.workspaces.insert(
                ws_b.clone(),
                WorkspaceState::new(crate::scope::Scope::default()),
            );
            st.invocations.insert("one".to_string(), owned(&ws_a));
            st.invocations.insert("two".to_string(), owned(&ws_b));
            st.invocations.get_mut("two").unwrap().process = None;
            st.invocations.get_mut("two").unwrap().process_exit_code = Some(0);
        }
        let first = core.dto();
        let second = core.dto();
        assert!(second.revision > first.revision);
        let json = serde_json::to_value(&second).unwrap();
        assert_eq!(json["workspaces"].as_array().unwrap().len(), 2);
        let invocations = json["invocations"].as_array().unwrap();
        assert_eq!(invocations.len(), 2);
        let finished = invocations
            .iter()
            .find(|invocation| invocation["id"] == "two")
            .unwrap();
        assert_eq!(finished["running"], false);
        assert_eq!(finished["stoppable"], false);
        assert_eq!(finished["processExitCode"], 0);
    }

    #[test]
    fn a_finished_invocation_keeps_its_console_readable() {
        let core = Core::new();
        let ws = PathBuf::from("/ws");
        {
            let mut st = core.state.lock().unwrap();
            st.workspaces.insert(
                ws.clone(),
                WorkspaceState::new(crate::scope::Scope::default()),
            );
            let mut invocation = owned(&ws);
            invocation.process = None;
            invocation.console = Some(crate::terminal::session_log().unwrap());
            st.invocations.insert("one".to_string(), invocation);
        }
        let json = serde_json::to_value(core.dto()).unwrap();
        let invocation = &json["invocations"][0];
        assert_eq!(invocation["console"], true);
        assert_eq!(invocation["running"], false);
    }

    fn fetch_state(state: &mut WorkspaceState) {
        state.ingest_request = Some("req-1".to_string());
        state.snapshot = Some(Arc::new(crate::snapshot::TargetIndex {
            meta: crate::snapshot::Meta {
                fetched_at: 1,
                status: crate::snapshot::Status::Ok,
            },
            targets: vec![],
        }));
        state.last_error = Some(IngestError {
            exit_code: Some(1),
            summary: "failed".to_string(),
        });
    }

    #[test]
    fn register_workspace_reuses_entries_and_keeps_fetch_state() {
        let core = Core::new();
        let ws = PathBuf::from("/ws");
        let scope = crate::scope::Scope::default();
        let mut st = core.state.lock().unwrap();
        st.register_workspace(ws.clone(), scope.clone());
        fetch_state(st.workspaces.get_mut(&ws).unwrap());
        // Same scope: the entry, request id, snapshot and error survive.
        st.register_workspace(ws.clone(), scope.clone());
        let state = &st.workspaces[&ws];
        assert_eq!(state.ingest_request.as_deref(), Some("req-1"));
        assert_eq!(state.snapshot.as_ref().unwrap().meta.fetched_at, 1);
        assert!(state.last_error.is_some());
    }

    #[test]
    fn register_workspace_scope_change_clears_fetch_state_only() {
        let core = Core::new();
        let ws = PathBuf::from("/ws");
        let other = PathBuf::from("/other");
        let scope = crate::scope::Scope::default();
        let mut st = core.state.lock().unwrap();
        st.register_workspace(ws.clone(), scope.clone());
        st.register_workspace(other.clone(), scope.clone());
        st.invocations.insert("run".to_string(), owned(&other));
        fetch_state(st.workspaces.get_mut(&other).unwrap());
        fetch_state(st.workspaces.get_mut(&ws).unwrap());
        // A different scope invalidates the cached fetch state only.
        st.register_workspace(
            ws.clone(),
            crate::scope::Scope {
                include: vec!["//app/...".to_string()],
            },
        );
        let state = &st.workspaces[&ws];
        assert!(state.snapshot.is_none());
        assert!(state.ingest_request.is_none());
        assert!(state.last_error.is_none());
        // The other workspace and owned invocations are untouched.
        assert_eq!(
            st.workspaces[&other]
                .snapshot
                .as_ref()
                .unwrap()
                .meta
                .fetched_at,
            1
        );
        assert!(st.invocations.contains_key("run"));
    }

    #[test]
    fn workspace_lookup_requires_the_registered_canonical_id() {
        let temp = std::env::temp_dir().join(format!("zeal-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&temp).unwrap();
        let canonical = temp.canonicalize().unwrap();
        let core = Core::new();
        assert!(
            core.state
                .lock()
                .unwrap()
                .workspace_path(temp.to_str().unwrap())
                .is_err()
        );
        {
            let mut st = core.state.lock().unwrap();
            st.register_workspace(canonical.clone(), crate::scope::Scope::default());
        }
        assert_eq!(
            core.state
                .lock()
                .unwrap()
                .workspace_path(canonical.to_str().unwrap())
                .unwrap(),
            canonical
        );
        // A different spelling of the same directory is not an id.
        let alias = temp.join("..").join(temp.file_name().unwrap());
        assert!(
            core.state
                .lock()
                .unwrap()
                .workspace_path(alias.to_str().unwrap())
                .is_err()
        );
        std::fs::remove_dir(&temp).unwrap();
    }
}
