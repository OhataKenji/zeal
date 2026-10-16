//! Workspace detection, bazel executable resolution, and persistence of the
//! last opened workspace.

use std::path::{Path, PathBuf};

/// A directory is a Bazel workspace root if it has MODULE.bazel or
/// WORKSPACE(.bazel).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "zeal-workspace-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn detects_each_workspace_marker() {
        for marker in ["MODULE.bazel", "WORKSPACE", "WORKSPACE.bazel"] {
            let dir = TempDir::new();
            fs::write(dir.0.join(marker), "").unwrap();
            assert!(is_workspace(&dir.0), "{marker}");
        }
    }

    #[test]
    fn rejects_dir_without_marker_file() {
        let dir = TempDir::new();
        assert!(!is_workspace(&dir.0));
        fs::write(dir.0.join("BUILD.bazel"), "").unwrap();
        assert!(!is_workspace(&dir.0));
        // A directory with a marker name does not count.
        fs::create_dir(dir.0.join("WORKSPACE")).unwrap();
        assert!(!is_workspace(&dir.0));
        assert!(!is_workspace(&dir.0.join("missing")));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new();
        let data_dir = dir.0.join("nested/data");
        let ws = Path::new("/some/ws with space/日本語");
        save(&data_dir, ws);
        assert_eq!(load_saved(&data_dir).as_deref(), Some(ws));
        let other = Path::new("/other/ws");
        save(&data_dir, other);
        assert_eq!(load_saved(&data_dir).as_deref(), Some(other));
    }

    #[test]
    fn load_saved_returns_none_for_missing_or_invalid_file() {
        let dir = TempDir::new();
        assert_eq!(load_saved(&dir.0), None);
        for body in ["{not json", "", "[]", "{}", r#"{"path": 1}"#] {
            fs::write(dir.0.join(SAVED_FILE), body).unwrap();
            assert_eq!(load_saved(&dir.0), None, "{body}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_requires_regular_file_with_exec_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let f = dir.0.join("tool");
        fs::write(&f, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable(&f));
        fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable(&f));
        // Directories have the exec bit but are not executables.
        assert!(!is_executable(&dir.0));
        assert!(!is_executable(&dir.0.join("missing")));
    }

    #[cfg(unix)]
    #[test]
    fn find_on_path_finds_sh_and_not_unknown_names() {
        let sh = find_on_path("sh").expect("sh on PATH");
        assert!(sh.ends_with("sh"));
        assert!(is_executable(&sh));
        assert_eq!(find_on_path("zeal-no-such-tool-on-path"), None);
    }
}
