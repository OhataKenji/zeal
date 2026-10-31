mod bep;
mod bes;
mod console;
mod ingest;
mod label;
mod profile;
mod protocol;
mod runner;
mod snapshot;
mod state;
mod store;
mod telemetry;
mod terminal;
mod workspace;
mod xml;

use ingest::{RunDetail, Section};
use profile::Profile;
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
#[tracing::instrument(target = "zeal::operations", name = "workspace.open", skip_all)]
fn open_workspace(
    app: AppHandle,
    core: State<Arc<Core>>,
    path: String,
) -> Result<AppStateDto, String> {
    let ws = expand_home(path.trim());
    if !ws.is_dir() {
        return Err(format!("not a directory: {}", ws.display()));
    }
    if !workspace::is_workspace(&ws) {
        return Err("no MODULE.bazel / WORKSPACE at this path".to_string());
    }
    let bazel = workspace::resolve_bazel();
    {
        let mut st = core.state.lock().expect("application state lock poisoned");
        // Keep the busy check and workspace assignment atomic with start
        // and query. Filesystem validation above holds no shared state.
        if st.run.is_some() || st.ingest_running {
            return Err("cannot change workspace while bazel is busy".to_string());
        }
        store::analytics::refresh_timezone();
        st.console = None;
        st.workspace = Some(ws.clone());
        st.bazel = bazel;
        // A different workspace has no shared snapshot with the previous one.
        st.snapshot = None;
        st.last_error = None;
        // The note can describe a run that was released without a confirmed
        // exit, and it belongs to the workspace that run was started in.
        st.run_note = None;
    }
    if let Ok(dir) = app.path().app_data_dir() {
        workspace::save(&dir, &ws);
    }
    ingest::request(app.clone(), core.inner().clone())?;
    Ok(core.dto())
}

/// Manual refresh: re-run the query unless bazel is already busy.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "workspace.refresh", skip_all)]
fn refresh(app: AppHandle, core: State<Arc<Core>>) -> Result<(), String> {
    ingest::request(app, core.inner().clone())
}

/// Start `bazel build|test|run <target>`. Fails while bazel is busy.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "run.start", skip_all)]
fn start_run(
    app: AppHandle,
    core: State<Arc<Core>>,
    cmd: String,
    target: String,
) -> Result<RunRecord, String> {
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    runner::start(app, core.inner().clone(), &data, &cmd, &target)
}

/// Stop the run in flight: SIGINT to its process group, then SIGKILL if it does
/// not exit. Does nothing when no run is held or a stop is already in flight.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "run.stop", skip_all)]
fn stop_run(app: AppHandle, core: State<Arc<Core>>) -> Result<(), String> {
    runner::stop(app, core.inner().clone())
}

#[tauri::command]
fn write_console(core: State<Arc<Core>>, id: String, data: Vec<u8>) -> Result<usize, String> {
    terminal::write(&core, &id, &data)
}

#[tauri::command]
fn resize_console(core: State<Arc<Core>>, id: String, cols: u16, rows: u16) -> Result<(), String> {
    terminal::resize(&core, &id, cols, rows)
}

#[tauri::command]
async fn read_console(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    id: String,
    offset: u64,
) -> Result<console::Output, String> {
    let core = core.inner().clone();
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        console::read(&data, &core, &id, offset).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "terminal read worker failed".to_string())?
}

/// Runs of the open workspace, newest first.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "runs.list", skip_all)]
fn list_runs(app: AppHandle, core: State<Arc<Core>>) -> Result<Vec<RunRecord>, String> {
    let Some(ws) = core
        .state
        .lock()
        .expect("application state lock poisoned")
        .workspace
        .clone()
    else {
        return Ok(Vec::new());
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::list(&data, &ws).map_err(|error| error.to_string())
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
        .expect("application state lock poisoned")
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
#[tracing::instrument(target = "zeal::operations", name = "run.detail.request", skip_all)]
async fn get_run(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    id: String,
) -> Result<RunDetail, String> {
    let id = uuid::Uuid::parse_str(&id)
        .map_err(|_| "invalid run id".to_string())?
        .to_string();
    let Some(ws) = core
        .state
        .lock()
        .expect("application state lock poisoned")
        .workspace
        .clone()
    else {
        return Err("no workspace".to_string());
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let worker = ArtifactWorker::acquire(&DETAIL_BUSY, "run detail worker busy")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _worker = worker;
        ingest::run_detail(&data, &ws, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "run detail worker failed".to_string())?
}

/// Return the saved timing profile and its availability status.
/// Reject the request if another profile read is active.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "run.profile.request", skip_all)]
async fn get_run_profile(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    id: String,
) -> Result<Section<Profile>, String> {
    let id = uuid::Uuid::parse_str(&id)
        .map_err(|_| "invalid run id".to_string())?
        .to_string();
    let Some(ws) = core
        .state
        .lock()
        .expect("application state lock poisoned")
        .workspace
        .clone()
    else {
        return Err("no workspace".to_string());
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let worker = ArtifactWorker::acquire(&PROFILE_BUSY, "profile worker busy")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _worker = worker;
        ingest::run_profile(&data, &ws, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "profile worker failed".to_string())?
}

/// Open a BUILD/bzl file at its recorded location with the OS default app.
#[tauri::command]
#[tracing::instrument(target = "zeal::operations", name = "file.open", skip_all)]
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
    // Reopen the last workspace, if it is still a workspace.
    if let Ok(dir) = app.path().app_data_dir()
        && let Some(ws) = workspace::load_saved(&dir).filter(|w| workspace::is_workspace(w))
    {
        {
            let mut st = core.state.lock().expect("application state lock poisoned");
            store::analytics::refresh_timezone();
            st.workspace = Some(ws);
            st.bazel = workspace::resolve_bazel();
        }
        let _ = ingest::request(app.handle().clone(), core);
    }
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
            telemetry::init(app);
            let core = Arc::new(Core::new());
            app.manage(core.clone());
            let data = app.path().app_data_dir().map_err(|error| error.to_string());
            let receiver_core = core.clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                bes::start(data, receiver_core.clone(), || receiver_core.emit(&handle)).await;
            });
            restore_workspace(app, core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            read_console,
            write_console,
            resize_console,
            open_workspace,
            refresh,
            start_run,
            stop_run,
            list_runs,
            get_analytics,
            get_run,
            get_run_profile,
            open_location
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
mod artifact_worker_tests {
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
