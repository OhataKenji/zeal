pub mod bep;
mod ingest;
pub mod label;
pub mod snapshot;
mod state;
pub mod store;
mod workspace;
pub mod xml;

use state::{AppStateDto, Core};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

#[tauri::command]
fn get_state(core: State<Arc<Core>>) -> AppStateDto {
    core.dto()
}

/// Open a directory as the Bazel workspace: validate it, resolve bazel, and
/// start a background snapshot ingestion.
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
        st.workspace = Some(ws.clone());
        st.bazel = bazel;
        // A different workspace has no shared snapshot with the previous one.
        st.snapshot = None;
        st.last_error = None;
    }
    if let Ok(dir) = app.path().app_data_dir() {
        workspace::save(&dir, &ws);
    }
    ingest::request(app.clone(), core.inner().clone());
    Ok(core.dto())
}

/// Manual refresh: re-run the query and swap the snapshot. Coalesced into one
/// follow-up run if an ingestion is already in flight.
#[tauri::command]
fn refresh(app: AppHandle, core: State<Arc<Core>>) {
    ingest::request(app, core.inner().clone());
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
                ingest::request(app.handle().clone(), core);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            open_workspace,
            refresh,
            open_location
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
