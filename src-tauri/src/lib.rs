mod bep;
mod ingest;
mod label;
mod profile;
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
use store::RunRecord;
use tauri::{AppHandle, Manager, State};

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
) -> Result<RunRecord, String> {
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    runner::start(app, core.inner().clone(), &data, &cmd, &target)
}

/// Runs of the open workspace, newest first.
#[tauri::command]
fn list_runs(app: AppHandle, core: State<Arc<Core>>) -> Result<Vec<RunRecord>, String> {
    let Some(ws) = core.state.lock().unwrap().workspace.clone() else {
        return Ok(Vec::new());
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(store::list(&data, &ws))
}

/// A run of the open workspace with its artifacts parsed.
/// Parse and serialize whole artifacts off the UI thread.
#[tauri::command(async)]
fn get_run(app: AppHandle, core: State<Arc<Core>>, id: String) -> Result<RunDetail, String> {
    let id = uuid::Uuid::parse_str(&id)
        .map_err(|_| "invalid run id".to_string())?
        .to_string();
    let Some(ws) = core.state.lock().unwrap().workspace.clone() else {
        return Err("no workspace".to_string());
    };
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    run_detail(&store::run_dir(&data, &ws, &id)).map_err(|e| e.to_string())
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
            app.manage(core.clone());
            // Reopen the last workspace, if it is still a workspace.
            if let Ok(dir) = app.path().app_data_dir()
                && let Some(ws) = workspace::load_saved(&dir).filter(|w| workspace::is_workspace(w))
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
            list_runs,
            get_run,
            open_location
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
