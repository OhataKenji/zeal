// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Set the process environment before Tauri starts any threads.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if fix_path_env::fix().is_err() {
        eprintln!("Could not load shell PATH");
    }
    zeal_lib::run();
}
