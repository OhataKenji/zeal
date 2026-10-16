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
/// canonicalized workspace path.
fn runs_dir(data: &Path, ws: &Path) -> PathBuf {
    let ws = canonical(ws);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in ws.to_string_lossy().bytes() {
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

fn read_record(dir: &Path) -> io::Result<RunRecord> {
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
