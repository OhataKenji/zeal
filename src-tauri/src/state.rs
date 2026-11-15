//! Shared application state held by the Rust core.
//!
//! One mutex guards workspace and owned-invocation maps plus the shutdown
//! flag; it serializes map ownership and process reap/signal decisions, not
//! admission capacity. Every Bazel operation Zeal starts gets its own
//! [`OwnedInvocation`] entry keyed by a fresh id, so a later launch never
//! overwrites an earlier one. External BES invocations are only observed and
//! are never inserted here.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
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
    /// The owned process group may still be signalled.
    pub stoppable: bool,
    /// A stop was accepted; it stays until the process leaves the entry.
    pub stopping: bool,
    /// Captured terminal output exists for this invocation.
    pub console: bool,
}

/// Result of registering a workspace: its canonical path plus the new state.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWorkspaceResult {
    pub workspace: String,
    pub state: AppStateDto,
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

/// Discovery scope for one registered workspace. It owns no execution
/// slot: Bazel operations live in [`CoreState::invocations`] instead, and
/// the target list itself is fetched on demand by `ingest`.
pub struct WorkspaceState {
    pub scope: crate::scope::Scope,
}

impl WorkspaceState {
    pub fn new(scope: crate::scope::Scope) -> Self {
        Self { scope }
    }
}

/// One locally launched Bazel operation. The entry exists exactly while its
/// worker owns the process, including the final capture drain; the worker's
/// release removes it.
pub struct OwnedInvocation {
    pub workspace: PathBuf,
    pub cmd: String,
    pub pattern: String,
    pub started_at: i64,
    pub process: ActiveInvocation,
    /// The session's captured terminal output; an already-unlinked temp file.
    /// The process slot is released only after the capture's final drain.
    pub console: Option<std::fs::File>,
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

    /// Register `ws` (already canonicalized) under `scope`, replacing any
    /// previous entry's scope without touching executions.
    pub fn register_workspace(&mut self, ws: PathBuf, scope: crate::scope::Scope) {
        self.workspaces.insert(ws, WorkspaceState::new(scope));
    }
}

/// Milliseconds since the Unix epoch, the store's timestamp convention.
pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// The DTO for one owned invocation in the aggregate state.
fn owned_dto(id: &str, invocation: &OwnedInvocation) -> OwnedInvocationDto {
    OwnedInvocationDto {
        id: id.to_string(),
        workspace: invocation.workspace.to_string_lossy().into_owned(),
        cmd: invocation.cmd.clone(),
        pattern: invocation.pattern.clone(),
        started_at: invocation.started_at,
        stoppable: invocation.process.pgid.is_some(),
        stopping: invocation.process.stop_started.is_some(),
        console: invocation.console.is_some(),
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

    /// Snapshot the state for the UI. The revision is assigned under the
    /// state lock; the UI discards older snapshots if delivery is reordered.
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
            process: ActiveInvocation::stub(Some(42)),
            console: None,
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
            st.invocations.get_mut("two").unwrap().process.pgid = None;
        }
        let first = core.dto();
        let second = core.dto();
        assert!(second.revision > first.revision);
        let json = serde_json::to_value(&second).unwrap();
        assert_eq!(json["workspaces"].as_array().unwrap().len(), 2);
        let invocations = json["invocations"].as_array().unwrap();
        assert_eq!(invocations.len(), 2);
        let reaped = invocations
            .iter()
            .find(|invocation| invocation["id"] == "two")
            .unwrap();
        assert_eq!(reaped["stoppable"], false);
        assert_eq!(reaped["stopping"], false);
    }

    #[test]
    fn dto_advertises_console_output() {
        let core = Core::new();
        let ws = PathBuf::from("/ws");
        {
            let mut st = core.state.lock().unwrap();
            st.workspaces.insert(
                ws.clone(),
                WorkspaceState::new(crate::scope::Scope::default()),
            );
            let mut invocation = owned(&ws);
            invocation.console = Some(crate::terminal::session_log().unwrap());
            st.invocations.insert("one".to_string(), invocation);
        }
        let json = serde_json::to_value(core.dto()).unwrap();
        let invocation = &json["invocations"][0];
        assert_eq!(invocation["console"], true);
        assert_eq!(invocation["stoppable"], true);
    }

    #[test]
    fn register_workspace_replaces_scope_without_touching_other_entries() {
        let core = Core::new();
        let ws = PathBuf::from("/ws");
        let other = PathBuf::from("/other");
        let scope = crate::scope::Scope::default();
        let mut st = core.state.lock().unwrap();
        st.register_workspace(ws.clone(), scope.clone());
        st.register_workspace(other.clone(), scope.clone());
        st.invocations.insert("run".to_string(), owned(&other));
        // Re-registering under a new scope replaces only that entry's scope.
        let narrow = crate::scope::Scope {
            include: vec!["//app/...".to_string()],
        };
        st.register_workspace(ws.clone(), narrow.clone());
        assert_eq!(st.workspaces[&ws].scope, narrow);
        assert_eq!(st.workspaces[&other].scope, scope);
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
