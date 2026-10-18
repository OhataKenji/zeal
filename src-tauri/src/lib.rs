mod bep;
mod ingest;
mod label;
mod profile;
mod runner;
mod snapshot;
mod state;
mod store;
mod telemetry;
mod workspace;
mod xml;

use ingest::{RunDetail, run_detail};
use state::{AppStateDto, Core};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use store::RunRecord;
use tauri::{AppHandle, Manager, RunEvent, State};

#[tauri::command]
fn get_state(core: State<Arc<Core>>) -> AppStateDto {
    core.dto()
}

/// Open a directory as the Bazel workspace: validate it, resolve bazel, and
/// start a background snapshot ingestion. Refused while a run is in flight: its
/// process group belongs to the workspace it was started in.
#[tauri::command]
fn open_workspace(
    app: AppHandle,
    core: State<Arc<Core>>,
    path: String,
    operation_id: Option<String>,
) -> Result<AppStateDto, String> {
    let mut operation = core
        .telemetry
        .start("workspace.open", operation_id.as_deref());
    operation.outcome("rejected");
    let ws = expand_home(path.trim());
    if !ws.is_dir() {
        return Err(format!("not a directory: {}", ws.display()));
    }
    if !workspace::is_workspace(&ws) {
        return Err("no MODULE.bazel / WORKSPACE at this path".to_string());
    }
    let bazel = workspace::resolve_bazel();
    {
        let mut st = core.state.lock().unwrap();
        // Keep the busy check and workspace assignment atomic with start
        // and query. Filesystem validation above holds no shared state.
        if st.run.is_some() || st.ingest_running {
            return Err("cannot change workspace while bazel is busy".to_string());
        }
        store::analytics::refresh_timezone();
        st.workspace = Some(ws.clone());
        st.bazel = bazel;
        // A different workspace has no shared snapshot with the previous one.
        st.snapshot = None;
        st.ingest_operation_id = None;
        st.last_error = None;
        // The note can describe a run that was released without a confirmed
        // exit, and it belongs to the workspace that run was started in.
        st.run_note = None;
    }
    if let Ok(dir) = app.path().app_data_dir() {
        workspace::save(&dir, &ws);
    }
    ingest::request(app.clone(), core.inner().clone(), operation)?;
    Ok(core.dto())
}

/// Manual refresh: re-run the query unless bazel is already busy.
#[tauri::command]
fn refresh(
    app: AppHandle,
    core: State<Arc<Core>>,
    operation_id: Option<String>,
) -> Result<(), String> {
    let operation = core
        .telemetry
        .start("workspace.refresh", operation_id.as_deref());
    ingest::request(app, core.inner().clone(), operation)
}

/// Start `bazel build|test <target>`. Fails while bazel is busy.
#[tauri::command]
fn start_run(
    app: AppHandle,
    core: State<Arc<Core>>,
    cmd: String,
    target: String,
    operation_id: Option<String>,
) -> Result<RunRecord, String> {
    let mut operation = core.telemetry.start("run", operation_id.as_deref());
    operation.outcome("rejected");
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    runner::start(app, core.inner().clone(), &data, &cmd, &target, operation)
}

/// Stop the run in flight: SIGINT to its process group, then SIGKILL if it does
/// not exit. Does nothing when no run is held or a stop is already in flight.
#[tauri::command]
fn stop_run(
    app: AppHandle,
    core: State<Arc<Core>>,
    operation_id: Option<String>,
) -> Result<(), String> {
    let mut operation = core.telemetry.start("run.stop", operation_id.as_deref());
    if let Some(id) = &core.state.lock().unwrap().run {
        operation.run_id(id);
    }
    operation.result(runner::stop(app, core.inner().clone()))
}

/// Runs of the open workspace, newest first.
#[tauri::command]
fn list_runs(
    app: AppHandle,
    core: State<Arc<Core>>,
    operation_id: Option<String>,
) -> Result<Vec<RunRecord>, String> {
    let mut operation = core.telemetry.start("runs.list", operation_id.as_deref());
    operation.outcome("rejected");
    let Some(ws) = core.state.lock().unwrap().workspace.clone() else {
        operation.outcome("ok");
        return Ok(Vec::new());
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let result = store::list(&data, &ws).map_err(|error| error.to_string());
    if let Ok(runs) = &result {
        operation.count("zeal.run_count", runs.len());
    }
    operation.result(result)
}

/// Aggregate only the core-owned workspace; caller cannot name a DB or root.
#[tauri::command]
async fn get_analytics(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    filter: store::analytics::Filter,
) -> Result<store::analytics::Analytics, String> {
    let ws = core
        .state
        .lock()
        .unwrap()
        .workspace
        .clone()
        .ok_or_else(|| "no workspace".to_string())?;
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        store::analytics::query(&data, &ws, filter).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "analytics worker failed".to_string())?
}

/// No queued profile parses. The guard stays with the blocking operation even
/// if the caller stops awaiting its response.
static DETAIL_BUSY: AtomicBool = AtomicBool::new(false);

struct DetailWorker;
impl DetailWorker {
    fn acquire() -> Result<Self, String> {
        DETAIL_BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "profile worker busy".to_string())?;
        Ok(Self)
    }
}
impl Drop for DetailWorker {
    fn drop(&mut self) {
        DETAIL_BUSY.store(false, Ordering::Release);
    }
}

/// Parse artifacts off the UI thread, with at most one detail operation active.
#[tauri::command]
async fn get_run(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    id: String,
    operation_id: Option<String>,
) -> Result<RunDetail, String> {
    let mut operation = core.telemetry.start("run.detail", operation_id.as_deref());
    operation.outcome("rejected");
    let id = uuid::Uuid::parse_str(&id)
        .map_err(|_| "invalid run id".to_string())?
        .to_string();
    let Some(ws) = core.state.lock().unwrap().workspace.clone() else {
        return Err("no workspace".to_string());
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let worker = DetailWorker::acquire()?;
    operation.run_id(&id);
    tauri::async_runtime::spawn_blocking(move || {
        let _worker = worker;
        let mut operation = operation;
        operation.outcome("unknown");
        let result = run_detail(&data, &ws, &id, &operation).map_err(|e| e.to_string());
        operation.result(result)
    })
    .await
    .map_err(|_| "profile worker failed".to_string())?
}

/// Open a BUILD/bzl file at its recorded location with the OS default app.
#[tauri::command]
fn open_location(path: String) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Only an allowlisted operation, outcome, UUID and bounded duration cross this boundary.
#[tauri::command]
fn record_ui_operation(core: State<Arc<Core>>, record: telemetry::UiOperation) {
    core.telemetry.record_ui(record);
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            let mut core = Core::new();
            core.telemetry = telemetry::Telemetry::from_env();
            let core = Arc::new(core);
            app.manage(core.clone());
            // Reopen the last workspace, if it is still a workspace.
            if let Ok(dir) = app.path().app_data_dir()
                && let Some(ws) = workspace::load_saved(&dir).filter(|w| workspace::is_workspace(w))
            {
                {
                    let mut st = core.state.lock().unwrap();
                    store::analytics::refresh_timezone();
                    st.workspace = Some(ws);
                    st.bazel = workspace::resolve_bazel();
                }
                let operation = core.telemetry.start("workspace.restore", None);
                let _ = ingest::request(app.handle().clone(), core, operation);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            open_workspace,
            refresh,
            start_run,
            stop_run,
            list_runs,
            get_analytics,
            get_run,
            open_location,
            record_ui_operation
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|handle, event| {
            // Bound stopping a live child; finish saving one already reaped.
            // `Exit` always precedes application termination.
            if let RunEvent::Exit = event {
                let core = handle.state::<Arc<Core>>().inner().clone();
                runner::exit_stop(&core);
                core.telemetry.shutdown();
            }
        });
}

#[cfg(test)]
mod detail_worker_tests {
    use super::DetailWorker;

    #[test]
    fn concurrent_detail_is_busy_until_worker_is_dropped() {
        let first = DetailWorker::acquire().unwrap();
        assert!(DetailWorker::acquire().is_err());
        drop(first);
        assert!(DetailWorker::acquire().is_ok());
    }
}
