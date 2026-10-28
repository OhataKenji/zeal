//! Workspace detection, bazel executable resolution, and persistence of the
//! last opened workspace.

use std::path::{Path, PathBuf};

/// A directory is a Bazel workspace root if it has MODULE.bazel or
/// WORKSPACE(.bazel).
#[must_use]
pub fn is_workspace(dir: &Path) -> bool {
    ["MODULE.bazel", "WORKSPACE", "WORKSPACE.bazel"]
        .iter()
        .any(|f| dir.join(f).is_file())
}

/// Prefer bazelisk on PATH (it respects `.bazelversion`), then bazel.
pub fn resolve_bazel() -> Option<PathBuf> {
    ["bazelisk", "bazel"].iter().find_map(|n| find_on_path(n))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path_var| {
        std::env::split_paths(&path_var)
            .map(|dir| dir.join(name))
            .find(|p| is_executable(p))
    })
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

const SAVED_FILE: &str = "workspace.json";

/// Read the persisted workspace path from `data_dir`, if any.
pub fn load_saved(data_dir: &Path) -> Option<PathBuf> {
    let s = std::fs::read_to_string(data_dir.join(SAVED_FILE)).ok()?;
    serde_json::from_str::<serde_json::Value>(&s)
        .ok()?
        .get("path")?
        .as_str()
        .map(PathBuf::from)
}

/// Persist the workspace path under `data_dir`. Best-effort.
pub fn save(data_dir: &Path, ws: &Path) {
    if std::fs::create_dir_all(data_dir).is_err() {
        return;
    }
    let body = serde_json::json!({ "path": ws.to_string_lossy() });
    let _ = std::fs::write(data_dir.join(SAVED_FILE), body.to_string());
}
