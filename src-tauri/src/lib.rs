mod actions;
mod bazel_output;
mod bep;
mod bes;
mod configured;
mod console;
mod detail;
mod ingest;
mod label;
mod modules;
mod perfetto;
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

use detail::InvocationDetail;
use query::QueryOutput;
use state::{AppStateDto, Core, OpenWorkspaceResult, RegisteredWorkspace};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};

/// Validate the invocation UUID, registered workspace, and app data directory
/// that every saved-invocation read command must pass identically.
fn saved_invocation_scope(
    app: &AppHandle,
    core: &Arc<Core>,
    workspace: &str,
    id: &str,
) -> Result<(PathBuf, PathBuf, String), String> {
    let id = uuid::Uuid::parse_str(id)
        .map_err(|_| "invalid invocation id".to_string())?
        .to_string();
    let ws = registered_workspace(core, workspace)?;
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok((data, ws, id))
}

#[tauri::command]
fn get_state(core: State<Arc<Core>>) -> AppStateDto {
    core.dto()
}

/// Fetch `bazel info` paths for a registered workspace.
#[tauri::command]
#[tracing::instrument(target = "zelith::operations", name = "workspace.info", skip_all)]
async fn get_workspace_info(
    core: State<'_, Arc<Core>>,
    workspace: String,
) -> Result<workspace::WorkspaceInfo, String> {
    let (bazel, ws_path) = {
        let st = core.state.lock().expect("application state lock poisoned");
        let ws_path = st.workspace_path(&workspace)?;
        let bazel = st.bazel.clone().ok_or_else(|| "no bazel".to_string())?;
        (bazel, ws_path)
    };
    tauri::async_runtime::spawn_blocking(move || workspace::fetch_info(&bazel, &ws_path))
        .await
        .map_err(|_| "workspace info worker failed".to_string())?
}

/// `bazel mod graph` for a registered workspace, under one per-workspace
/// fail-fast guard.
#[tauri::command]
#[tracing::instrument(target = "zelith::operations", name = "modules.get", skip_all)]
async fn get_modules(
    core: State<'_, Arc<Core>>,
    workspace: String,
) -> Result<modules::ModulesSnapshot, String> {
    let op = modules::ModulesOperation::acquire(core.inner(), &workspace)?;
    tauri::async_runtime::spawn_blocking(move || op.snapshot())
        .await
        .map_err(|_| "modules worker failed".to_string())?
}

/// `bazel mod explain --verbose` for one module key in a registered
/// workspace, under the same per-workspace fail-fast guard.
#[tauri::command]
#[tracing::instrument(target = "zelith::operations", name = "modules.explain", skip_all)]
async fn explain_module(
    core: State<'_, Arc<Core>>,
    workspace: String,
    module_key: String,
) -> Result<modules::ModuleExplanation, String> {
    let op = modules::ModulesOperation::acquire(core.inner(), &workspace)?;
    tauri::async_runtime::spawn_blocking(move || op.explain(&module_key))
        .await
        .map_err(|_| "modules worker failed".to_string())?
}

/// Load relationships for one target in `workspace`.
#[tauri::command]
#[tracing::instrument(
    target = "zelith::operations",
    name = "target.detail.request",
    skip_all
)]
async fn get_target_detail(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    label: String,
) -> Result<snapshot::TargetDetail, String> {
    let core = core.inner().clone();
    let emit_core = core.clone();
    let emit_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        ingest::target_detail(app, core, &workspace, &label)
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

/// One inspector read step (configuration, transitions, outputs, or
/// actions) for `label` in `workspace`, resolved under the applied build
/// flags. Each call runs at most one Bazel process.
#[tauri::command]
#[tracing::instrument(target = "zelith::operations", name = "target.read.request", skip_all)]
async fn read_target(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    label: String,
    flags: String,
    step: ingest::TargetRead,
) -> Result<ingest::TargetReadData, String> {
    let core = core.inner().clone();
    let emit_core = core.clone();
    let emit_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        ingest::read_target(app, core, &workspace, &label, &flags, step)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            emit_core.emit(&emit_app);
            Err("target read worker failed".to_string())
        }
    }
}

/// Register a directory as a Bazel workspace: validate it, canonicalize it,
/// reuse or create its state entry, and save it as the last-workspace
/// preference. It does not query; the caller refreshes explicitly. Running
/// invocations in any workspace are unaffected.
#[tauri::command]
#[tracing::instrument(target = "zelith::operations", name = "workspace.open", skip_all)]
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
    let ws = canonical_workspace_path(&ws)?;
    let bazel = workspace::resolve_bazel();
    let data = app.path().app_data_dir().ok();
    let registration = workspace_registration(&ws, scope.clone(), data.as_deref());
    {
        let mut st = core.state.lock().expect("application state lock poisoned");
        store::analytics::refresh_timezone();
        st.workspaces.insert(ws.clone(), registration);
        st.bazel = bazel;
        st.last_workspace = Some(ws.clone());
    }
    if let Some(dir) = data {
        workspace::save(&dir, &ws, &scope);
    }
    core.emit(&app);
    Ok(OpenWorkspaceResult {
        workspace: ws.to_string_lossy().into_owned(),
        state: core.dto(),
    })
}

/// Manual refresh: run the target-list query for `workspace` under `scope`
/// and return its result. Every call is an independent owned invocation.
#[tauri::command]
#[tracing::instrument(target = "zelith::operations", name = "workspace.refresh", skip_all)]
async fn refresh(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    scope: scope::Scope,
) -> Result<ingest::TargetLoad, String> {
    let core = core.inner().clone();
    let emit_core = core.clone();
    let emit_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        ingest::request(app, core, &workspace, scope)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            emit_core.emit(&emit_app);
            Err("target list worker failed".to_string())
        }
    }
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
#[tracing::instrument(target = "zelith::operations", name = "invocation.start", skip_all)]
fn start_invocation(
    app: AppHandle,
    core: State<Arc<Core>>,
    workspace: String,
    cmd: String,
    pattern: String,
    arguments: Option<String>,
) -> Result<String, String> {
    let ws = registered_workspace(&core, &workspace)?;
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
#[tracing::instrument(target = "zelith::operations", name = "invocation.stop", skip_all)]
fn stop_invocation(
    app: AppHandle,
    core: State<Arc<Core>>,
    workspace: String,
    id: String,
) -> Result<(), String> {
    let ws = registered_workspace(&core, &workspace)?;
    runner::stop(app, core.inner().clone(), &ws, &id)
}

/// Run an ad-hoc `bazel query` from the query editor as its own owned
/// invocation. The blocking work is spawned so this command does not hold
/// the main thread for the whole query.
#[tauri::command]
#[tracing::instrument(target = "zelith::operations", name = "query.exec", skip_all)]
async fn run_query(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    request: query::QueryRequest,
) -> Result<QueryOutput, String> {
    let core = core.inner().clone();
    let emit_core = core.clone();
    let emit_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || query::run(app, core, &request)).await {
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

/// The saved build-event history for one workspace plus the workspace's
/// bounded in-memory diagnostics for terminal completions that never
/// produced a readable record. A store failure stays visible in `saved`
/// without hiding the local diagnostics.
#[derive(serde::Serialize)]
struct InvocationHistory {
    saved: Result<Vec<InvocationRecord>, String>,
    diagnostics: Vec<state::ProcessDiagnostic>,
}

/// Recent history plus live and selected records within the workspace's
/// scope, and its local diagnostics — returned even while a live entry
/// still owns the process, since a history read may precede the release
/// event; the projection alone resolves precedence — all validated
/// against the registered workspace.
#[tauri::command]
#[tracing::instrument(target = "zelith::operations", name = "invocations.list", skip_all)]
async fn list_invocations(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    selected_id: Option<String>,
) -> Result<InvocationHistory, String> {
    let ws = registered_workspace(&core, &workspace)?;
    let core = core.inner().clone();
    let data = app.path().app_data_dir().map_err(|error| error.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        load_invocation_history(&core, &ws, selected_id, data)
    })
    .await
    .map_err(|_| "history list worker failed".to_string())
}

/// The synchronous history read run inside the worker: snapshot the live
/// and selected pins plus this workspace's diagnostic ids under the state
/// lock, read the store without it, then lock again to retire diagnostics
/// whose record was fetched and scope the reply. Diagnostic ids are
/// lookup-only pins — they can pull an off-window record back for the
/// retirement check but are never exposed as history rows themselves.
fn load_invocation_history(
    core: &Core,
    ws: &Path,
    selected_id: Option<String>,
    data: Result<PathBuf, String>,
) -> InvocationHistory {
    let (visible_pins, lookup_pins) = {
        let state = core.state.lock().expect("application state lock poisoned");
        let mut visible_pins = state.invocations.keys().cloned().collect::<Vec<_>>();
        visible_pins.extend(selected_id);
        let mut lookup_pins = visible_pins.clone();
        lookup_pins.extend(
            state
                .diagnostics
                .iter()
                .filter(|diagnostic| Path::new(&diagnostic.workspace) == ws)
                .map(|diagnostic| diagnostic.id.clone()),
        );
        (visible_pins, lookup_pins)
    };
    let mut saved =
        data.and_then(|dir| store::list(&dir, ws, &lookup_pins).map_err(|error| error.to_string()));
    let mut state = core.state.lock().expect("application state lock poisoned");
    settle_invocation_history(&mut state, &mut saved, &visible_pins);
    InvocationHistory {
        saved,
        diagnostics: state.diagnostics_for(ws),
    }
}

/// Whether the fetched records hold this workspace's own record for the
/// diagnostic's invocation id.
fn owns_diagnostic_record(
    records: &[InvocationRecord],
    diagnostic: &state::ProcessDiagnostic,
) -> bool {
    records.iter().any(|record| {
        record.id == diagnostic.id
            && record.workspace.as_deref() == Some(diagnostic.workspace.as_str())
    })
}

/// With the store read done: a fetched record owned by this workspace
/// retires its diagnostic — only on success — and records beyond the
/// recent window stay in the reply only for visible live or selected pins.
fn settle_invocation_history(
    state: &mut state::CoreState,
    saved: &mut Result<Vec<InvocationRecord>, String>,
    visible_pins: &[String],
) {
    let Ok(records) = saved else {
        return;
    };
    state
        .diagnostics
        .retain(|diagnostic| !owns_diagnostic_record(records, diagnostic));
    if records.len() > store::LIST_LIMIT {
        let extras = records.split_off(store::LIST_LIMIT);
        records.extend(
            extras
                .into_iter()
                .filter(|record| visible_pins.contains(&record.id)),
        );
    }
}

/// Aggregate only a registered workspace; caller cannot name a DB or root.
#[tauri::command]
async fn get_analytics(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    filter: store::analytics::Filter,
) -> Result<store::analytics::Analytics, String> {
    let ws = registered_workspace(&core, &workspace)?;
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        store::analytics::query(&data, &ws, filter).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "analytics worker failed".to_string())?
}

/// Return the saved record, log and build results.
#[tauri::command]
#[tracing::instrument(
    target = "zelith::operations",
    name = "invocation.detail.request",
    skip_all
)]
async fn get_invocation(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    workspace: String,
    id: String,
) -> Result<InvocationDetail, String> {
    let (data, ws, id) = saved_invocation_scope(&app, core.inner(), &workspace, &id)?;
    tauri::async_runtime::spawn_blocking(move || {
        detail::invocation_detail(&data, &ws, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "invocation detail worker failed".to_string())?
}

/// Timing-tab entry point: validate the saved artifact, confirm the bundled
/// viewer is installed, then issue a one-shot grant and return the viewer
/// URL carrying only its random token — never the workspace or invocation id.
#[tauri::command]
#[tracing::instrument(
    target = "zelith::operations",
    name = "invocation.timing.request",
    skip_all
)]
async fn prepare_invocation_timing(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    pending: State<'_, perfetto::PendingTrace>,
    workspace: String,
    id: String,
) -> Result<String, String> {
    let (data, ws, id) = saved_invocation_scope(&app, core.inner(), &workspace, &id)?;
    perfetto::asset_root(&app)?;
    // Reserve the grant before the first await so a newer selection can
    // supersede this one while the artifact is still opening.
    let token = pending.begin();
    let file = tauri::async_runtime::spawn_blocking(move || {
        profile::open(&data, &ws, &id).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => {
                "No saved timing profile is available for this invocation.".to_string()
            }
            _ => e.to_string(),
        })
    })
    .await
    .map_err(|_| "profile worker failed".to_string())??;
    if !pending.finish(token, file) {
        return Err("Timing selection changed.".into());
    }
    Ok(format!(
        "{}/embed.html#trace/{token}/profile.json.gz",
        perfetto::VIEWER_ORIGIN
    ))
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

/// Canonicalize a workspace root and reject one the String path API cannot
/// represent, before it is registered or persisted.
fn canonical_workspace_path(path: &Path) -> Result<PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
    path.to_str().ok_or("workspace path must be valid UTF-8")?;
    Ok(path)
}

fn registered_workspace(core: &Core, workspace: &str) -> Result<PathBuf, String> {
    core.state
        .lock()
        .expect("application state lock poisoned")
        .workspace_path(workspace)
}

/// Sample the origin at open/restore, outside the core lock. The saved
/// history store and live workspace registry use the same literal.
fn workspace_registration(
    ws: &Path,
    scope: scope::Scope,
    data: Option<&Path>,
) -> RegisteredWorkspace {
    let repo_url = workspace::repo_url(ws);
    if let Some(data) = data {
        let _ = store::refresh_workspace(data, ws, repo_url.as_deref());
    }
    RegisteredWorkspace { scope, repo_url }
}

fn restore_workspace(app: &tauri::App, core: Arc<Core>) {
    // Re-register the last workspace, if it is still a workspace. Its saved
    // scope restores with it; the caller refreshes explicitly.
    if let Ok(dir) = app.path().app_data_dir()
        && let Some(saved) =
            workspace::load_saved(&dir).filter(|w| workspace::is_workspace(&w.path))
        && let Ok(ws) = canonical_workspace_path(&saved.path)
    {
        let registration = workspace_registration(&ws, saved.scope, Some(&dir));
        {
            let mut st = core.state.lock().expect("application state lock poisoned");
            store::analytics::refresh_timezone();
            st.workspaces.insert(ws.clone(), registration);
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
        .register_asynchronous_uri_scheme_protocol("perfetto", |context, request, responder| {
            let app = context.app_handle().clone();
            let main_webview = context.webview_label() == "main";
            tauri::async_runtime::spawn(async move {
                responder.respond(perfetto::handle(&app, main_webview, &request).await);
            });
        })
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
            app.manage(perfetto::PendingTrace::default());
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
            get_workspace_info,
            get_modules,
            explain_module,
            get_target_detail,
            read_target,
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
            prepare_invocation_timing
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
    use super::canonical_workspace_path;

    #[cfg(unix)]
    #[test]
    fn canonical_workspace_path_resolves_a_symlink() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!("zelith-ws-path-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let child = dir.join("child");
        std::fs::create_dir(&child).unwrap();
        let alias = dir.join("alias");
        symlink(&child, &alias).unwrap();
        let resolved = canonical_workspace_path(&alias).unwrap();
        assert_eq!(resolved, child.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn canonical_workspace_path_rejects_non_utf8_roots() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!("zelith-ws-path-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let bad = dir.join(OsString::from_vec(b"bad\xff".to_vec()));
        std::fs::create_dir(&bad).unwrap();
        let alias = dir.join("alias");
        symlink(&bad, &alias).unwrap();
        let error = canonical_workspace_path(&alias).unwrap_err();
        assert_eq!(error, "workspace path must be valid UTF-8");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
