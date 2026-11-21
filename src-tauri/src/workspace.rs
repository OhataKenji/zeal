//! Workspace detection, bazel executable resolution, and persistence of the
//! last opened workspace.

use crate::scope::Scope;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const INFO_TIMEOUT: Duration = Duration::from_secs(30);
const INFO_STDOUT_CAP: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub workspace: String,
    pub release: Option<String>,
    pub output_base: Option<String>,
    pub execution_root: Option<String>,
    pub bazel_bin: Option<String>,
    pub bazel_testlogs: Option<String>,
    pub repository_cache: Option<String>,
}

/// Pure parser for `bazel info` `key: value` output.
#[must_use]
pub fn parse_bazel_info(stdout: &str, workspace: &Path) -> WorkspaceInfo {
    let mut info = WorkspaceInfo {
        workspace: workspace.to_string_lossy().into_owned(),
        release: None,
        output_base: None,
        execution_root: None,
        bazel_bin: None,
        bazel_testlogs: None,
        repository_cache: None,
    };
    for line in stdout.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let val = val.trim();
        if val.is_empty() {
            continue;
        }
        match key.trim() {
            "release" => info.release = Some(val.to_owned()),
            "output_base" => info.output_base = Some(val.to_owned()),
            "execution_root" => info.execution_root = Some(val.to_owned()),
            "bazel-bin" => info.bazel_bin = Some(val.to_owned()),
            "bazel-testlogs" => info.bazel_testlogs = Some(val.to_owned()),
            "repository_cache" => info.repository_cache = Some(val.to_owned()),
            _ => {}
        }
    }
    info
}

/// Run `bazel --noblock_for_lock info` in `ws` with bounded IO and timeout.
pub fn fetch_info(bazel: &Path, ws: &Path) -> Result<WorkspaceInfo, String> {
    fetch_info_with_timeout(bazel, ws, INFO_TIMEOUT)
}

fn fetch_info_with_timeout(
    bazel: &Path,
    ws: &Path,
    timeout: Duration,
) -> Result<WorkspaceInfo, String> {
    let out = crate::bazel_output::run_bazel_bounded(
        bazel,
        ws,
        &["info"],
        Instant::now() + timeout,
        INFO_STDOUT_CAP,
        // Workspace info has no lifecycle owner; it is never cancelled.
        &AtomicBool::new(false),
    )?;
    Ok(parse_bazel_info(&String::from_utf8_lossy(&out.stdout), ws))
}

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

#[must_use]
pub fn repo_url(ws: &Path) -> Option<String> {
    repo_url_from(
        std::process::Command::new("git")
            .arg("-C")
            .arg(ws)
            .args(["remote", "get-url", "origin"]),
    )
}

fn repo_url_from(command: &mut std::process::Command) -> Option<String> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8(output.stdout).ok()?;
    let url = url.trim_end_matches(['\r', '\n']);
    (!url.is_empty()).then(|| url.to_owned())
}

const SAVED_FILE: &str = "workspace.json";

/// The last workspace and its explicitly selected discovery scope.
#[derive(Deserialize)]
pub struct SavedWorkspace {
    pub path: PathBuf,
    #[serde(default)]
    pub scope: Scope,
}

/// Legacy path-only settings use the default scope; invalid settings are not restored.
pub fn load_saved(data_dir: &Path) -> Option<SavedWorkspace> {
    let s = std::fs::read_to_string(data_dir.join(SAVED_FILE)).ok()?;
    let mut saved: SavedWorkspace = serde_json::from_str(&s).ok()?;
    saved.scope = saved.scope.validated().ok()?;
    Some(saved)
}

/// Persist the workspace and scope under `data_dir`. Best-effort.
pub fn save(data_dir: &Path, ws: &Path, scope: &Scope) {
    if std::fs::create_dir_all(data_dir).is_err() {
        return;
    }
    let body = serde_json::json!({ "path": ws.to_string_lossy(), "scope": scope });
    let _ = std::fs::write(data_dir.join(SAVED_FILE), body.to_string());
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    /// A unique temporary directory removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("zeal-workspace-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn git(dir: &Path, args: &[&str]) -> Output {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    pub fn init(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "--quiet"]);
        git(
            dir,
            &[
                "-c",
                "user.email=zeal@example.com",
                "-c",
                "user.name=zeal",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
    }

    pub fn repo(dir: &Path, origin: &str) {
        init(dir);
        git(dir, &["remote", "add", "origin", origin]);
    }

    /// Write an executable shell script standing in for the bazel binary.
    pub fn fake_bazel(dir: &Path, name: &str, script: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::TempDir;
    use super::*;

    #[test]
    fn persisted_scope_survives_reopening() {
        let dir = TempDir::new();
        let scope = Scope {
            include: vec!["//app/...".into()],
        };
        save(dir.path(), Path::new("/workspace"), &scope);
        let saved = load_saved(dir.path()).unwrap();
        assert_eq!(saved.path, Path::new("/workspace"));
        assert_eq!(saved.scope, scope);
    }

    #[test]
    fn legacy_settings_restore_with_the_default_scope() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join(SAVED_FILE), r#"{"path":"/workspace"}"#).unwrap();
        let saved = load_saved(dir.path()).unwrap();
        assert_eq!(saved.path, Path::new("/workspace"));
        assert_eq!(saved.scope, Scope::default());
    }

    #[test]
    fn invalid_explicit_scope_is_not_replaced_with_the_default() {
        let dir = TempDir::new();
        for scope in [
            serde_json::Value::Null,
            serde_json::json!("//..."),
            serde_json::json!({"include": []}),
            serde_json::json!({"include": ["//app\\unsafe"]}),
        ] {
            let settings = serde_json::json!({"path": "/workspace", "scope": scope});
            std::fs::write(dir.path().join(SAVED_FILE), settings.to_string()).unwrap();
            assert!(load_saved(dir.path()).is_none(), "{settings}");
        }
    }

    #[test]
    fn old_exclude_settings_are_ignored() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join(SAVED_FILE), r#"{"path":"/workspace","scope":{"include":["//app/..."],"exclude":["//app/old/..."]}}"#).unwrap();
        let saved = load_saved(dir.path()).unwrap();
        assert_eq!(saved.scope.include, ["//app/..."]);
        assert_eq!(saved.scope.rules_query(), "kind(\"rule\", (\"//app/...\"))");
    }

    #[test]
    fn repo_url_returns_the_origin_literal_without_its_trailing_newline() {
        let dir = TempDir::new();
        let repo = dir.path().join("repo");
        test_support::repo(&repo, "git@example.com:2222/org/re.po.git");
        assert_eq!(
            repo_url(&repo).as_deref(),
            Some("git@example.com:2222/org/re.po.git")
        );
    }

    #[test]
    fn clones_and_worktrees_share_the_same_literal_origin() {
        let dir = TempDir::new();
        let origin = "https://example.com/org/shared.git";
        let source = dir.path().join("source");
        test_support::repo(&source, origin);
        let clone = dir.path().join("clone");
        test_support::git(
            dir.path(),
            &[
                "clone",
                "--quiet",
                source.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        test_support::git(&clone, &["remote", "set-url", "origin", origin]);
        let worktree = dir.path().join("worktree");
        test_support::git(
            &source,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                worktree.to_str().unwrap(),
            ],
        );
        for ws in [&source, &clone, &worktree] {
            assert_eq!(repo_url(ws).as_deref(), Some(origin), "{ws:?}");
        }
    }

    #[test]
    fn repo_url_is_none_without_origin_or_repository() {
        let dir = TempDir::new();
        let no_origin = dir.path().join("no-origin");
        test_support::init(&no_origin);
        assert_eq!(repo_url(&no_origin), None);
        let plain = dir.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        assert_eq!(repo_url(&plain), None);
        assert_eq!(repo_url(&dir.path().join("missing")), None);
    }

    #[test]
    fn repo_url_is_none_when_git_cannot_run() {
        let mut command = std::process::Command::new("/nonexistent/zeal-git");
        command
            .arg("-C")
            .arg("/")
            .args(["remote", "get-url", "origin"]);
        assert_eq!(repo_url_from(&mut command), None);
    }

    #[test]
    fn parses_bazel_info_keys_and_ignores_unknown() {
        let out = "\
release: release 8.7.0
output_base: /private/var/tmp/_bazel_dev/a9f3e12c
execution_root: /private/var/tmp/_bazel_dev/a9f3e12c/execroot/_main
bazel-bin: /private/var/tmp/_bazel_dev/a9f3e12c/execroot/_main/bazel-out/darwin_arm64-fastbuild/bin
bazel-testlogs: /private/var/tmp/_bazel_dev/a9f3e12c/execroot/_main/bazel-out/darwin_arm64-fastbuild/testlogs
repository_cache: /Users/dev/Library/Caches/bazel:repo_cache
unknown_key: ignored
empty_val:
";
        let info = parse_bazel_info(out, Path::new("/demo/ws"));
        assert_eq!(info.workspace, "/demo/ws");
        assert_eq!(info.release.as_deref(), Some("release 8.7.0"));
        assert_eq!(
            info.output_base.as_deref(),
            Some("/private/var/tmp/_bazel_dev/a9f3e12c")
        );
        assert_eq!(
            info.repository_cache.as_deref(),
            Some("/Users/dev/Library/Caches/bazel:repo_cache")
        );
        assert!(info.bazel_bin.is_some());
        assert!(info.bazel_testlogs.is_some());
        assert!(info.execution_root.is_some());
    }
}
