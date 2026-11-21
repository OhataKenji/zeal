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

mod configs;

const INFO_TIMEOUT: Duration = Duration::from_secs(30);
const INFO_STDOUT_CAP: usize = 64 * 1024;
const INFO_STDERR_HEAD: usize = 128 * 1024;
const INFO_STDERR_TAIL: usize = 4 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RcAnnouncement {
    pub source: String,
    pub scope: String,
    pub flags: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEntry {
    pub command: String,
    pub flags: String,
    pub source: String,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BazelConfig {
    pub name: String,
    pub entries: Vec<ConfigEntry>,
}

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
    pub rc_announcements: Vec<RcAnnouncement>,
    pub configs: Vec<BazelConfig>,
    pub info_error: Option<String>,
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
        rc_announcements: Vec::new(),
        configs: Vec::new(),
        info_error: None,
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

/// Drain the pipe even after the retained prefix has filled. Keep up to three
/// extra bytes at each cut so decoding a split UTF-8 scalar is lossless.
fn read_capped(mut reader: impl Read, head: usize, tail: usize) -> (String, String) {
    let mut prefix = Vec::new();
    let mut suffix = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }
        let take = n.min(head + 3 - prefix.len());
        prefix.extend_from_slice(&buf[..take]);
        if tail > 0 {
            suffix.extend_from_slice(&buf[..n]);
            suffix.drain(..suffix.len().saturating_sub(tail + 3));
        }
    }
    let prefix = String::from_utf8_lossy(&prefix);
    let suffix = String::from_utf8_lossy(&suffix);
    let mut end = head.min(prefix.len());
    while !prefix.is_char_boundary(end) {
        end -= 1;
    }
    let mut start = suffix.len().saturating_sub(tail);
    while !suffix.is_char_boundary(start) {
        start += 1;
    }
    (prefix[..end].to_owned(), suffix[start..].to_owned())
}

/// Config definitions don't depend on whether Bazel is installed or can run.
pub fn fetch_info(bazel: Option<&Path>, ws: &Path) -> WorkspaceInfo {
    let result = bazel
        .ok_or_else(|| "no bazel".to_owned())
        .and_then(|bazel| fetch_info_with_timeout(bazel, ws, INFO_TIMEOUT));
    let mut info = result.unwrap_or_else(|error| {
        let mut info = parse_bazel_info("", ws);
        info.info_error = Some(error);
        info
    });
    info.configs = configs::load_workspace_configs(ws);
    info
}

fn fetch_info_with_timeout(
    bazel: &Path,
    ws: &Path,
    timeout: Duration,
) -> Result<WorkspaceInfo, String> {
    let mut cmd = Command::new(bazel);
    cmd.args(["--noblock_for_lock", "info", "--announce_rc"])
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
        let _ = out_tx.send(read_capped(out_pipe, INFO_STDOUT_CAP, 0).0);
    });
    let (err_tx, err_rx) = mpsc::channel();
    let err_pipe = child.stderr.take().expect("stderr piped");
    thread::spawn(move || {
        let _ = err_tx.send(read_capped(err_pipe, INFO_STDERR_HEAD, INFO_STDERR_TAIL));
    });

    let status = wait_with_timeout(&mut child, pid, Instant::now() + timeout)?;

    let stdout = out_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    let (head, tail) = err_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    let mut info = parse_bazel_info(if status.success() { &stdout } else { "" }, ws);
    info.rc_announcements = parse_announce_rc(&head);
    if !status.success() {
        let detail = tail.trim();
        info.info_error = Some(if detail.is_empty() {
            format!("bazel info exited with {status}")
        } else {
            detail.to_owned()
        });
    }
    Ok(info)
}

fn parse_announce_rc(stderr: &str) -> Vec<RcAnnouncement> {
    let mut active_source = None;
    stderr
        .lines()
        .filter_map(|line| announcement(line, &mut active_source))
        .collect()
}

fn announcement<'a>(line: &'a str, active: &mut Option<&'a str>) -> Option<RcAnnouncement> {
    if !line.starts_with(char::is_whitespace) {
        *active = None;
    }
    let line = line.trim();
    if let Some(header) = line.strip_prefix("INFO: Reading rc options for '") {
        let (_, path) = header.split_once("' from ")?;
        *active = path.strip_suffix(':');
        return None;
    }
    announcement_options(line, *active)
}

fn announcement_options(line: &str, active: Option<&str>) -> Option<RcAnnouncement> {
    let (source, scope, flags) = if let Some(startup) =
        line.strip_prefix("INFO: Reading 'startup' options from ")
    {
        let (source, flags) = startup.split_once(": ")?;
        (source, "startup".to_owned(), flags)
    } else if let Some(config) = line.strip_prefix("INFO: Found applicable config definition ") {
        let (config, rest) = config.split_once(" in file ")?;
        let (_, name) = config.split_once(':')?;
        let (source, flags) = rest.split_once(": ")?;
        (source, format!("config:{name}"), flags)
    } else {
        let options = line.strip_prefix("Inherited ").unwrap_or(line);
        let (scope, flags) = options.strip_prefix('\'')?.split_once("' options: ")?;
        (active?, scope.to_owned(), flags)
    };
    Some(RcAnnouncement {
        source: source.to_owned(),
        scope,
        flags: flags.to_owned(),
    })
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
    use std::path::{Path, PathBuf};
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
}

#[cfg(test)]
mod tests {
    use super::test_support::TempDir;
    use super::*;

    #[test]
    fn parses_multiline_rc_sections_without_attributing_client_flags() {
        let lines = "\
INFO: Reading 'startup' options from /my repo/.bazelrc: --batch
INFO: Reading rc options for 'info' from /my repo/.bazelrc:
  Inherited 'common' options: --color=no
  'info' options: --config=ci
INFO: Options provided by the client:
  Inherited 'common' options: --isatty=1
INFO: Found applicable config definition info:ci in file /my repo/rc/team.rc: --curses=no
";
        let parsed = parse_announce_rc(lines);
        let values: Vec<_> = parsed
            .iter()
            .map(|a| (a.source.as_str(), a.scope.as_str(), a.flags.as_str()))
            .collect();
        assert_eq!(
            values,
            [
                ("/my repo/.bazelrc", "startup", "--batch"),
                ("/my repo/.bazelrc", "common", "--color=no"),
                ("/my repo/.bazelrc", "info", "--config=ci"),
                ("/my repo/rc/team.rc", "config:ci", "--curses=no"),
            ]
        );
    }

    #[test]
    fn multibyte_stderr_is_trimmed_at_scalar_boundaries() {
        let data = "界".repeat(INFO_STDERR_HEAD / 3 + INFO_STDERR_TAIL);
        let (head, tail) = read_capped(data.as_bytes(), INFO_STDERR_HEAD, INFO_STDERR_TAIL);
        assert_eq!(head, "界".repeat(INFO_STDERR_HEAD / 3));
        assert_eq!(tail, "界".repeat(INFO_STDERR_TAIL / 3));
    }

    #[test]
    fn missing_bazel_still_loads_configs() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join(".bazelrc"), "build:ci -c opt").unwrap();
        let info = fetch_info(None, dir.path());
        assert_eq!(info.configs[0].name, "ci");
        assert_eq!(info.info_error.as_deref(), Some("no bazel"));
        let info = fetch_info(Some(&dir.path().join("nonexistent")), dir.path());
        assert_eq!(info.configs[0].name, "ci");
        assert!(info.info_error.unwrap().contains("failed to start"));
    }

    #[cfg(unix)]
    #[test]
    fn fetch_preserves_early_announcements_and_late_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let script = dir.path().join("bazel");
        let create_script = |body: &str| {
            std::fs::write(&script, format!("#!/bin/sh\n[ \"$*\" = '--noblock_for_lock info --announce_rc' ] || exit 99\n{body}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        std::fs::write(
            dir.path().join(".bazelrc"),
            "test:ci --flaky_test_attempts=2",
        )
        .unwrap();
        create_script(
            "echo 'release: fake'\necho \"INFO: Reading 'startup' options from /rc: --batch\" >&2",
        );
        let info = fetch_info(Some(&script), dir.path());
        assert_eq!(info.release.as_deref(), Some("fake"));
        assert_eq!(info.rc_announcements[0].flags, "--batch");
        assert!(info.info_error.is_none());

        create_script(
            "echo \"INFO: Reading 'startup' options from /rc: --batch\" >&2\nhead -c 150000 /dev/zero | tr '\\000' x >&2\nprintf '\\nlock held\\n' >&2\nexit 7",
        );
        let info = fetch_info(Some(&script), dir.path());
        let error = info.info_error.unwrap();
        assert!(error.ends_with("lock held"));
        assert!(error.len() <= INFO_STDERR_TAIL);
        assert_eq!(info.configs[0].name, "ci");
        assert_eq!(info.rc_announcements[0].flags, "--batch");
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
