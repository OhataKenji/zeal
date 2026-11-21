//! Workspace detection, bazel executable resolution, and persistence of the
//! last opened workspace.

use crate::scope::Scope;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const INFO_TIMEOUT: Duration = Duration::from_secs(30);
const INFO_STDOUT_CAP: usize = 64 * 1024;
const INFO_STDERR_CAP: usize = 8 * 1024;

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

fn read_capped<R: Read>(mut reader: R, cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }
        if out.len() < cap {
            let take = n.min(cap - out.len());
            out.extend_from_slice(&buf[..take]);
        }
    }
    out
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
    let mut cmd = Command::new(bazel);
    cmd.args(["--noblock_for_lock", "info"])
        .current_dir(ws)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start bazel info: {e}"))?;
    let pid = child.id();

    let (out_tx, out_rx) = mpsc::channel();
    let out_pipe = child.stdout.take().expect("stdout piped");
    thread::spawn(move || {
        let _ = out_tx.send(read_capped(out_pipe, INFO_STDOUT_CAP));
    });
    let (err_tx, err_rx) = mpsc::channel();
    let err_pipe = child.stderr.take().expect("stderr piped");
    thread::spawn(move || {
        let _ = err_tx.send(read_capped(err_pipe, INFO_STDERR_CAP));
    });

    let status = wait_with_timeout(&mut child, pid, Instant::now() + timeout)?;

    let stdout = out_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    let stderr = err_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("bazel info exited with {status}")
        } else {
            detail
        });
    }
    Ok(parse_bazel_info(&String::from_utf8_lossy(&stdout), ws))
}

fn wait_with_timeout(
    child: &mut std::process::Child,
    pid: u32,
    deadline: Instant,
) -> Result<std::process::ExitStatus, String> {
    loop {
        if let Some(status) = child.try_wait().map_err(|e| {
            kill_group_and_reap(child, pid);
            format!("failed waiting for bazel info: {e}")
        })? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            kill_group_and_reap(child, pid);
            return Err("bazel info timed out".to_string());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn kill_group_and_reap(child: &mut std::process::Child, pid: u32) {
    #[cfg(unix)]
    if let Ok(pgid) = i32::try_from(pid)
        && pgid > 1
    {
        // SAFETY: pgid > 1 targets only the child's own process group created via process_group(0).
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
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
    use std::path::Path;
    use std::process::{Command, Output};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("zeal-workspace-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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
