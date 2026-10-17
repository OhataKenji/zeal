//! Run storage. Each run gets a dir under a per-workspace data dir:
//! `<data>/ws-<fnv64(workspace)>/runs/<uuid>/` with `run.json` (the record) and
//! `log.txt` (stdout and stderr). Bazel writes its artifacts next to them.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The content of `run.json`. The status (running / ok / failed / crashed) is
/// not stored; the UI derives it from `finished_at` and `exit_code`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub workspace: String,
    pub cmd: String,
    pub target: String,
    /// Unix time in milliseconds.
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// `<data>/ws-<hex>/runs`, where `<hex>` is the FNV-1a 64-bit hash of the
/// canonicalized workspace path. The hash input is the raw OS byte string, not
/// the lossy string, so distinct non-UTF-8 paths cannot collide.
fn runs_dir(data: &Path, ws: &Path) -> PathBuf {
    let ws = canonical(ws);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in ws.as_os_str().as_encoded_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    data.join(format!("ws-{h:016x}")).join("runs")
}

fn canonical(ws: &Path) -> PathBuf {
    ws.canonicalize().unwrap_or_else(|_| ws.to_path_buf())
}

pub fn run_dir(data: &Path, ws: &Path, id: &str) -> PathBuf {
    runs_dir(data, ws).join(id)
}

/// Create the run dir with a new `run.json` and an empty `log.txt`. Returns
/// the record and the run dir, which the other functions take.
pub fn create(data: &Path, ws: &Path, cmd: &str, target: &str) -> io::Result<(RunRecord, PathBuf)> {
    let ws = canonical(ws);
    let rec = RunRecord {
        id: uuid::Uuid::new_v4().to_string(),
        workspace: ws.to_string_lossy().into_owned(),
        cmd: cmd.to_string(),
        target: target.to_string(),
        started_at: now_ms(),
        finished_at: None,
        exit_code: None,
    };
    let dir = run_dir(data, &ws, &rec.id);
    fs::create_dir_all(&dir)?;
    write_record(&dir, &rec)?;
    fs::write(dir.join("log.txt"), "")?;
    Ok((rec, dir))
}

pub fn append_log(dir: &Path, line: &str) -> io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(dir.join("log.txt"))?;
    writeln!(f, "{line}")
}

/// Record the end of a run in its `run.json`.
pub fn finish(dir: &Path, exit_code: i32) -> io::Result<RunRecord> {
    let mut rec = read_record(dir)?;
    rec.finished_at = Some(now_ms());
    rec.exit_code = Some(exit_code);
    write_record(dir, &rec)?;
    Ok(rec)
}

/// All runs of the workspace, newest first. Unreadable `run.json` are skipped.
pub fn list(data: &Path, ws: &Path) -> Vec<RunRecord> {
    let Ok(entries) = fs::read_dir(runs_dir(data, ws)) else {
        return Vec::new();
    };
    let mut runs: Vec<RunRecord> = entries
        .flatten()
        .filter_map(|e| read_record(&e.path()).ok())
        .collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    runs
}

pub fn read_log(dir: &Path) -> io::Result<String> {
    fs::read_to_string(dir.join("log.txt"))
}

pub fn read_record(dir: &Path) -> io::Result<RunRecord> {
    let s = fs::read_to_string(dir.join("run.json"))?;
    serde_json::from_str(&s).map_err(io::Error::other)
}

fn write_record(dir: &Path, rec: &RunRecord) -> io::Result<()> {
    let s = serde_json::to_string(rec).map_err(io::Error::other)?;
    fs::write(dir.join("run.json"), s)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A fresh `(data dir, workspace dir)` pair; the caller removes the root.
    fn temp_dirs(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("zeal-store-{name}-{}-{nanos}", std::process::id()));
        let (data, ws) = (root.join("data"), root.join("ws"));
        std::fs::create_dir_all(&ws).unwrap();
        (root, data, ws)
    }

    #[test]
    fn create_log_finish_list_round_trip() {
        let (root, data, ws) = temp_dirs("round-trip");

        let (rec, dir) = create(&data, &ws, "build", "//pkg:a").unwrap();
        assert_eq!(dir, run_dir(&data, &ws, &rec.id));
        assert_eq!(rec.workspace, ws.canonicalize().unwrap().to_string_lossy());
        assert_eq!((rec.finished_at, rec.exit_code), (None, None));
        assert_eq!(read_log(&dir).unwrap(), "");

        append_log(&dir, "INFO: Analyzed target //pkg:a").unwrap();
        append_log(&dir, "INFO: Build completed").unwrap();
        let done = finish(&dir, 1).unwrap();
        assert_eq!(done.exit_code, Some(1));
        assert!(done.finished_at.unwrap() >= rec.started_at);

        assert_eq!(list(&data, &ws), vec![done]);
        assert_eq!(
            read_log(&dir).unwrap(),
            "INFO: Analyzed target //pkg:a\nINFO: Build completed\n"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_json_uses_camel_case_keys() {
        let (root, data, ws) = temp_dirs("json-keys");

        let (_, dir) = create(&data, &ws, "build", "//pkg:a").unwrap();
        finish(&dir, 0).unwrap();
        let json = std::fs::read_to_string(dir.join("run.json")).unwrap();
        for key in [
            "\"id\"",
            "\"workspace\"",
            "\"cmd\"",
            "\"target\"",
            "\"startedAt\"",
            "\"finishedAt\"",
            "\"exitCode\"",
        ] {
            assert!(json.contains(key), "{key} missing in {json}");
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_is_newest_first_with_unfinished_runs_and_skips_broken() {
        let (root, data, ws) = temp_dirs("list");

        // Pin started_at so ordering does not depend on the wall clock.
        let (mut old, old_dir) = create(&data, &ws, "build", "//pkg:a").unwrap();
        old.started_at = 1;
        write_record(&old_dir, &old).unwrap();
        let old = finish(&old_dir, 0).unwrap();

        // Left without finish(), like a run whose app crashed.
        let (mut unfinished, unfinished_dir) = create(&data, &ws, "test", "//pkg:t").unwrap();
        unfinished.started_at = 2;
        write_record(&unfinished_dir, &unfinished).unwrap();

        let (_, broken_dir) = create(&data, &ws, "build", "//pkg:b").unwrap();
        std::fs::write(broken_dir.join("run.json"), "{").unwrap();

        let runs = list(&data, &ws);
        assert_eq!(runs, vec![unfinished, old]);
        assert_eq!(runs[0].finished_at, None);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_ascii_workspace_round_trip() {
        let (root, data, _) = temp_dirs("non-ascii");
        let ws = root.join("開発-ワークスペース");
        std::fs::create_dir_all(&ws).unwrap();

        let (rec, dir) = create(&data, &ws, "build", "//pkg:a").unwrap();
        assert_eq!(dir, run_dir(&data, &ws, &rec.id));
        assert_eq!(rec.workspace, ws.canonicalize().unwrap().to_string_lossy());
        assert_eq!(list(&data, &ws), vec![rec]);

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Unix paths may hold non-UTF-8 bytes. Two distinct workspaces whose lossy
    /// strings are identical must still get distinct run dirs. (macOS rejects such
    /// names at mkdir, so the dirs are never created; `run_dir` falls back to the
    /// given path when canonicalize fails.)
    #[cfg(unix)]
    #[test]
    fn non_utf8_workspaces_do_not_collide() {
        use std::os::unix::ffi::OsStrExt;

        let (root, data, _) = temp_dirs("non-utf8");
        // Both lossy-convert to "<root>/\u{FFFD}".
        let ws_a = root.join(std::ffi::OsStr::from_bytes(b"\xff"));
        let ws_b = root.join(std::ffi::OsStr::from_bytes(b"\xfe"));

        assert_ne!(runs_dir(&data, &ws_a), runs_dir(&data, &ws_b));

        std::fs::remove_dir_all(root).unwrap();
    }
}
