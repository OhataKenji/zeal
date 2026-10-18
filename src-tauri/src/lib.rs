#[cfg(test)]
mod acceptance;
mod bes;
mod ingest;
mod label;
mod profile;
mod protocol;
mod runner;
mod snapshot;
mod state;
mod store;
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
) -> Result<AppStateDto, String> {
    let ws = expand_home(path.trim());
    if !ws.is_dir() {
        return Err(format!("not a directory: {}", ws.display()));
    }
    if !workspace::is_workspace(&ws) {
        return Err("no MODULE.bazel / WORKSPACE at this path".to_string());
    }
    let ws = ws
        .canonicalize()
        .map_err(|_| "workspace canonicalization failed".to_string())?;
    let bazel = workspace::resolve_bazel();
    {
        let mut st = core.state.lock().unwrap();
        // Keep the busy check and workspace assignment atomic with start
        // and query. Filesystem validation above holds no shared state.
        if st.run.is_some() || st.ingest_running {
            return Err("cannot change workspace while bazel is busy".to_string());
        }
        st.workspace = Some(ws.clone());
        st.bazel = bazel;
        // A different workspace has no shared snapshot with the previous one.
        st.snapshot = None;
        st.last_error = None;
        // The note can describe a run that was released without a confirmed
        // exit, and it belongs to the workspace that run was started in.
        st.run_note = None;
        st.completed_runs.clear();
        st.process_exit = None;
    }
    if let Ok(dir) = app.path().app_data_dir() {
        workspace::save(&dir, &ws);
    }
    ingest::request(app.clone(), core.inner().clone())?;
    Ok(core.dto())
}

/// Manual refresh: re-run the query unless bazel is already busy.
#[tauri::command]
fn refresh(app: AppHandle, core: State<Arc<Core>>) -> Result<(), String> {
    ingest::request(app, core.inner().clone())
}

/// Start `bazel build|test <target>`. Fails while bazel is busy.
#[tauri::command]
fn start_run(
    app: AppHandle,
    core: State<Arc<Core>>,
    cmd: String,
    target: String,
) -> Result<String, String> {
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    runner::start(app, core.inner().clone(), &data, &cmd, &target)
}

/// Stop the run in flight: SIGINT to its process group, then SIGKILL if it does
/// not exit. Does nothing when no run is held or a stop is already in flight.
#[tauri::command]
fn stop_run(app: AppHandle, core: State<Arc<Core>>) -> Result<(), String> {
    runner::stop(app, core.inner().clone())
}

/// Runs of the open workspace, newest first.
#[tauri::command]
fn list_runs(app: AppHandle, core: State<Arc<Core>>) -> Result<Vec<RunRecord>, String> {
    let Some(ws) = core.state.lock().unwrap().workspace.clone() else {
        return Ok(Vec::new());
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::list(&data, &ws).map_err(|e| e.to_string())
}

/// One bounded detail worker. Reject concurrent requests before scheduling work.
static DETAIL_BUSY: AtomicBool = AtomicBool::new(false);

struct DetailWorker;
impl Drop for DetailWorker {
    fn drop(&mut self) {
        DETAIL_BUSY.store(false, Ordering::Release);
    }
}

#[tauri::command]
async fn get_run(
    app: AppHandle,
    core: State<'_, Arc<Core>>,
    id: String,
) -> Result<RunDetail, String> {
    let id = uuid::Uuid::parse_str(&id)
        .map_err(|_| "invalid run id".to_string())?
        .to_string();
    let ws = core
        .state
        .lock()
        .unwrap()
        .workspace
        .clone()
        .ok_or("no workspace")?;
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    DETAIL_BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "profile worker busy".to_string())?;
    let worker = DetailWorker;
    tauri::async_runtime::spawn_blocking(move || {
        let _worker = worker;
        run_detail(&data, &ws, &id)
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
            let core = Arc::new(Core::new());
            let data = app.path().app_data_dir()?;
            if let Err(error) = store::ensure_ready(&data) {
                core.state.lock().unwrap().run_note = Some(error.to_string());
            }
            let server = bes::Server::start(core.clone(), data)?;
            core.state.lock().unwrap().bes_port = Some(server.port);
            app.manage(server);
            app.manage(core.clone());
            // Reopen the last workspace, if it is still a workspace.
            if let Ok(dir) = app.path().app_data_dir()
                && let Some(ws) = workspace::load_saved(&dir)
                    .and_then(|w| w.canonicalize().ok())
                    .filter(|w| workspace::is_workspace(w))
            {
                {
                    let mut st = core.state.lock().unwrap();
                    st.workspace = Some(ws);
                    st.bazel = workspace::resolve_bazel();
                }
                let _ = ingest::request(app.handle().clone(), core);
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
            get_run,
            open_location
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|handle, event| {
            // A run in flight must not be orphaned, and the exit cannot wait for
            // a reap: one bounded stop (SIGINT, a short poll, SIGKILL) as the
            // process goes. `Exit` is the event that always precedes it.
            if let RunEvent::Exit = event {
                let core = handle.state::<Arc<Core>>().inner().clone();
                runner::exit_stop(&core);
            }
        });
}
