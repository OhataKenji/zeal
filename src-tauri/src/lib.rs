mod actions;
mod bep;
mod bes;
mod console;
mod detail;
mod ingest;
mod label;
mod profile;
mod protocol;
mod query;
mod query_proto;
mod runargs;
mod runner;
mod scope;
mod snapshot;
mod state;
mod store;
mod telemetry;
mod terminal;
mod workspace;

// Public boundaries used by external clients and integration tests.
pub use bes::serve as serve_build_events;
pub use protocol::{bep as build_event_protocol, bes as build_event_service_protocol};
pub use store::{
    ErrorKind, InvocationRecord, StoreError, artifact as open_invocation_artifact,
    list as list_invocation_records, read_record as read_invocation_record,
};

use detail::{InvocationDetail, Section};
use profile::Profile;
use query::QueryOutput;
use state::{AppStateDto, Core, OpenWorkspaceResult, OwnedInvocationDto};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};

#[tauri::command]
fn get_state(core: State<Arc<Core>>) -> AppStateDto {
    core.dto()
}

/// The target index of one registered workspace. Events carry only
/// `snapshot_fetched_at`, so the UI requests this once per change instead of
/// on every state push.
#[tracing::instrument(target = "zeal::operations", name = "snapshot.get", skip_all)]
#[tauri::command]
async fn get_snapshot(
    core: State<'_, Arc<Core>>,
    workspace: String,
) -> Result<Option<Arc<snapshot::TargetIndex>>, String> {
    let st = core.state.lock().expect("application state lock poisoned");
    let ws = st.workspace_path(&workspace)?;
    Ok(st
        .workspaces
        .get(&ws)
        .and_then(|state| state.snapshot.clone()))
}

/// Load relationships for one target in the workspace's target-list
/// generation.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "target.detail.request", skip_all)]
async fn get_target_detail(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    label: String,
    fetched_at: u64,
) -> Result<snapshot::TargetDetail, String> {
    let core = core.inner().clone();
    let emit_core = core.clone();
    let emit_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        ingest::target_detail(app, core, &workspace, &label, fetched_at)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            emit_core.emit(&emit_app);
            Err("target detail worker failed".to_string())
        }
    }
}

/// The selected target's output files, resolved by `bazel cquery`.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "target.outputs.request", skip_all)]
async fn get_target_outputs(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    label: String,
    fetched_at: u64,
) -> Result<ingest::TargetOutputs, String> {
    let core = core.inner().clone();
    let emit_core = core.clone();
    let emit_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        ingest::target_outputs(app, core, &workspace, &label, fetched_at)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            emit_core.emit(&emit_app);
            Err("target outputs worker failed".to_string())
        }
    }
}

#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "target.actions.request", skip_all)]
async fn get_target_actions(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    label: String,
    fetched_at: u64,
) -> Result<actions::TargetActions, String> {
    let core = core.inner().clone();
    let emit_core = core.clone();
    let emit_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        ingest::target_actions(app, core, &workspace, &label, fetched_at)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            emit_core.emit(&emit_app);
            Err("target actions worker failed".to_string())
        }
    }
}

/// Register a directory as a Bazel workspace: validate it, canonicalize it,
/// reuse or create its state entry, and save it as the last-workspace
/// preference. It does not query; the caller refreshes explicitly. Running
/// invocations in any workspace are unaffected.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "workspace.open", skip_all)]
fn open_workspace(
    app: AppHandle,
    core: State<Arc<Core>>,
    path: String,
    scope: scope::Scope,
) -> Result<OpenWorkspaceResult, String> {
    let scope = scope.validated()?;
    let ws = expand_home(path.trim());
    if !ws.is_dir() {
        return Err(format!("not a directory: {}", ws.display()));
    }
    if !workspace::is_workspace(&ws) {
        return Err("no MODULE.bazel / WORKSPACE at this path".to_string());
    }
    let ws = ws
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", ws.display()))?;
    let bazel = workspace::resolve_bazel();
    {
        let mut st = core.state.lock().expect("application state lock poisoned");
        store::analytics::refresh_timezone();
        st.register_workspace(ws.clone(), scope.clone());
        st.bazel = bazel;
        st.last_workspace = Some(ws.clone());
    }
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = store::refresh_workspace(&dir, &ws);
        workspace::save(&dir, &ws, &scope);
    }
    core.emit(&app);
    Ok(OpenWorkspaceResult {
        workspace: ws.to_string_lossy().into_owned(),
        state: core.dto(),
    })
}

/// Manual refresh: re-run the target-list query for `workspace` unless that
/// workspace already has one in flight.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "workspace.refresh", skip_all)]
fn refresh(app: AppHandle, core: State<Arc<Core>>, workspace: String) -> Result<(), String> {
    ingest::request(app, core.inner().clone(), &workspace)
}

#[tauri::command]
async fn list_workspaces(app: AppHandle) -> Result<Vec<PathBuf>, String> {
    let data = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        store::frequent_workspaces(&data).map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "workspace list worker failed".to_string())?
}

/// Start `bazel build|test|run <target>` in `workspace`.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "invocation.start", skip_all)]
fn start_invocation(
    app: AppHandle,
    core: State<Arc<Core>>,
    workspace: String,
    cmd: String,
    pattern: String,
    arguments: Option<String>,
) -> Result<OwnedInvocationDto, String> {
    let ws = {
        let st = core.state.lock().expect("application state lock poisoned");
        st.workspace_path(&workspace)?
    };
    runner::start(
        app,
        core.inner().clone(),
        &ws,
        &cmd,
        &pattern,
        arguments.as_deref().unwrap_or_default(),
    )
}

/// Stop the owned invocation `id` in `workspace`.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "invocation.stop", skip_all)]
fn stop_invocation(
    app: AppHandle,
    core: State<Arc<Core>>,
    workspace: String,
    id: String,
) -> Result<(), String> {
    let ws = {
        let st = core.state.lock().expect("application state lock poisoned");
        st.workspace_path(&workspace)?
    };
    runner::stop(app, core.inner().clone(), &ws, &id)
}

/// Run an ad-hoc `bazel query` from the query editor as its own owned
/// invocation. The blocking work is spawned so this command does not hold
/// the main thread for the whole query.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "query.exec", skip_all)]
async fn run_query(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    expr: String,
    output: String,
    no_implicit_deps: bool,
    no_tool_deps: bool,
) -> Result<QueryOutput, String> {
    let core = core.inner().clone();
    let emit_core = core.clone();
    let emit_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        query::run(
            app,
            core,
            &workspace,
            &expr,
            &output,
            no_implicit_deps,
            no_tool_deps,
        )
    })
    .await
    {
        Ok(result) => result,
        // A panicked worker released its slot without emitting (Drop skips
        // emit while unwinding); tell the UI the query state changed here.
        Err(_) => {
            emit_core.emit(&emit_app);
            Err("query worker failed".to_string())
        }
    }
}

#[tauri::command]
fn write_console(
    core: State<Arc<Core>>,
    workspace: String,
    id: String,
    data: Vec<u8>,
) -> Result<usize, String> {
    terminal::write(&core, &workspace, &id, &data)
}

#[tauri::command]
fn resize_console(
    core: State<Arc<Core>>,
    workspace: String,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    terminal::resize(&core, &workspace, &id, cols, rows)
}

#[tauri::command]
async fn read_console(
    core: State<'_, Arc<Core>>,
    workspace: String,
    id: String,
    offset: u64,
) -> Result<console::Output, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        console::read(&core, &workspace, &id, offset).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "terminal read worker failed".to_string())?
}

/// Invocations of `workspace`, newest first, at most [`store::LIST_LIMIT`].
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "invocations.list", skip_all)]
async fn list_invocations(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
) -> Result<Vec<InvocationRecord>, String> {
    let ws = {
        let st = core.state.lock().expect("application state lock poisoned");
        st.workspace_path(&workspace)?
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        store::list(&data, &ws).map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "history list worker failed".to_string())?
}

/// Aggregate only a registered workspace; caller cannot name a DB or root.
#[tauri::command]
async fn get_analytics(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    filter: store::analytics::Filter,
) -> Result<store::analytics::Analytics, String> {
    let ws = {
        let st = core.state.lock().expect("application state lock poisoned");
        st.workspace_path(&workspace)?
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        store::analytics::query(&data, &ws, filter).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "analytics worker failed".to_string())?
}

static DETAIL_BUSY: AtomicBool = AtomicBool::new(false);
static PROFILE_BUSY: AtomicBool = AtomicBool::new(false);

/// A non-queuing permit for one active artifact read per flag.
/// The blocking worker owns it until completion, even if its caller stops waiting.
struct ArtifactWorker(&'static AtomicBool);
impl ArtifactWorker {
    fn acquire(busy: &'static AtomicBool, error: &str) -> Result<Self, String> {
        busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| error.to_string())?;
        Ok(Self(busy))
    }
}
impl Drop for ArtifactWorker {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Return the saved record, log and build results.
/// Reject the request if another detail read is active.
#[tauri::command]
#[tracing::instrument(
    target = "zeal::operations",
    name = "invocation.detail.request",
    skip_all
)]
async fn get_invocation(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    id: String,
) -> Result<InvocationDetail, String> {
    let id = uuid::Uuid::parse_str(&id)
        .map_err(|_| "invalid invocation id".to_string())?
        .to_string();
    let ws = {
        let st = core.state.lock().expect("application state lock poisoned");
        st.workspace_path(&workspace)?
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let worker = ArtifactWorker::acquire(&DETAIL_BUSY, "invocation detail worker busy")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _worker = worker;
        detail::invocation_detail(&data, &ws, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "invocation detail worker failed".to_string())?
}

/// Return the saved timing profile and its availability status.
/// Reject the request if another profile read is active.
#[tauri::command]
#[tracing::instrument(
    target = "zeal::operations",
    name = "invocation.profile.request",
    skip_all
)]
async fn get_invocation_profile(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    id: String,
) -> Result<Section<Profile>, String> {
    let id = uuid::Uuid::parse_str(&id)
        .map_err(|_| "invalid invocation id".to_string())?
        .to_string();
    let ws = {
        let st = core.state.lock().expect("application state lock poisoned");
        st.workspace_path(&workspace)?
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let worker = ArtifactWorker::acquire(&PROFILE_BUSY, "profile worker busy")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _worker = worker;
        detail::invocation_profile(&data, &ws, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "profile worker failed".to_string())?
}

/// `~/...` in a typed path expands to the user's home directory.
fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return Path::new(&home).join(rest);
    }
    PathBuf::from(path)
}

fn restore_workspace(app: &tauri::App, core: Arc<Core>) {
    // Re-register the last workspace, if it is still a workspace. Its saved
    // scope restores with it; the caller refreshes explicitly.
    if let Ok(dir) = app.path().app_data_dir()
        && let Some(saved) =
            workspace::load_saved(&dir).filter(|w| workspace::is_workspace(&w.path))
        && let Ok(ws) = saved.path.canonicalize()
    {
        let _ = store::refresh_workspace(&dir, &ws);
        {
            let mut st = core.state.lock().expect("application state lock poisoned");
            store::analytics::refresh_timezone();
            st.register_workspace(ws.clone(), saved.scope);
            st.last_workspace = Some(ws);
        }
    }
}

fn emit_invocations_changed(app: &AppHandle) {
    if let Err(error) = app.emit("invocations-changed", ()) {
        log::warn!("emit invocations-changed failed: {error}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            telemetry::init(app);
            let core = Arc::new(Core::new());
            app.manage(core.clone());
            let data = app.path().app_data_dir().map_err(|error| error.to_string());
            let receiver_core = core.clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let invocations_handle = handle.clone();
                bes::start(
                    data,
                    receiver_core.clone(),
                    || receiver_core.emit(&handle),
                    move || emit_invocations_changed(&invocations_handle),
                )
                .await;
            });
            restore_workspace(app, core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_snapshot,
            get_target_detail,
            get_target_outputs,
            get_target_actions,
            read_console,
            write_console,
            resize_console,
            open_workspace,
            refresh,
            list_workspaces,
            start_invocation,
            stop_invocation,
            run_query,
            list_invocations,
            get_analytics,
            get_invocation,
            get_invocation_profile
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|handle, event| {
            // Bound stopping a live child; finish saving one already reaped.
            // `Exit` always precedes application termination.
            if let RunEvent::Exit = event {
                let core = handle.state::<Arc<Core>>().inner().clone();
                runner::exit_stop(&core);
                telemetry::shutdown(handle);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{ArtifactWorker, DETAIL_BUSY, PROFILE_BUSY};

    #[test]
    fn readers_are_independent_and_each_stays_busy_until_its_worker_is_dropped() {
        let detail = ArtifactWorker::acquire(&DETAIL_BUSY, "detail busy").unwrap();
        let profile = ArtifactWorker::acquire(&PROFILE_BUSY, "profile busy").unwrap();
        assert!(ArtifactWorker::acquire(&DETAIL_BUSY, "detail busy").is_err());
        assert!(ArtifactWorker::acquire(&PROFILE_BUSY, "profile busy").is_err());
        drop(detail);
        assert!(ArtifactWorker::acquire(&DETAIL_BUSY, "detail busy").is_ok());
        assert!(ArtifactWorker::acquire(&PROFILE_BUSY, "profile busy").is_err());
        drop(profile);
        assert!(ArtifactWorker::acquire(&PROFILE_BUSY, "profile busy").is_ok());
    }
}
