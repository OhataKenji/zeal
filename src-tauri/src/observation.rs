//! Read models shared by owned executions and externally observed builds.
use crate::{bep, ingest::Section, store};
use serde::Serialize;
use std::{
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    pub finished_at: Option<u64>,
    pub exit_code: Option<i32>,
    pub complete: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunLive {
    pub record: store::RunRecord,
    pub log: Section<String>,
    pub bep: Section<bep::Bep>,
    pub build: Option<BuildResult>,
}

pub fn read(data: &Path, workspace: &Path, id: &str) -> io::Result<RunLive> {
    let record = store::read_record(data, workspace, id)?;
    let observed = store::events::get(data, workspace, id)?;
    let log = if record.external {
        let observed = observed
            .as_ref()
            .expect("external record has an observation");
        log_section(observed.log.clone(), observed.log_truncated)
    } else {
        saved_log(data, workspace, id)
    };
    let bep = if let Some(observed) = observed.as_ref() {
        if observed.bep.truncated {
            Section::Incomplete {
                data: observed.bep.clone(),
                note: "BES result display reached its 8 MiB limit; original events are retained."
                    .into(),
            }
        } else {
            Section::Ok {
                data: observed.bep.clone(),
            }
        }
    } else {
        match store::artifact(data, workspace, id, "bep.json") {
            Ok(file) => {
                let capped = file.metadata()?.len() > 4 * 1024 * 1024;
                let bep = bep::parse(io::BufReader::new(file.take(4 * 1024 * 1024)));
                if bep.truncated || capped {
                    Section::Incomplete {
                        data: bep,
                        note: "BEP is partial; live file reads are limited to 4 MiB.".into(),
                    }
                } else {
                    Section::Ok { data: bep }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Section::Missing,
            Err(error) => Section::ReadError {
                note: error.to_string(),
            },
        }
    };
    let build = observed.map(|observed| BuildResult {
        finished_at: observed.finished_at,
        exit_code: observed.exit_code,
        complete: observed.complete,
    });
    Ok(RunLive {
        record,
        log,
        bep,
        build,
    })
}

pub fn saved_log(data: &Path, workspace: &Path, id: &str) -> Section<String> {
    match tail(data, workspace, id) {
        Ok((text, truncated)) => log_section(text, truncated),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Section::Missing,
        Err(error) => Section::ReadError {
            note: error.to_string(),
        },
    }
}

fn log_section(data: String, truncated: bool) -> Section<String> {
    if truncated {
        Section::Incomplete {
            data,
            note: "Showing the last 512 KiB; the full output is saved locally.".into(),
        }
    } else {
        Section::Ok { data }
    }
}

fn tail(data: &Path, workspace: &Path, id: &str) -> io::Result<(String, bool)> {
    let mut file = store::artifact(data, workspace, id, "log.txt")?;
    let end = file.metadata()?.len();
    let start = end.saturating_sub(store::events::LOG_LIMIT as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(end - start).read_to_end(&mut bytes)?;
    let first = if start > 0 {
        bytes
            .iter()
            .position(|b| b & 0xc0 != 0x80)
            .unwrap_or(bytes.len())
    } else {
        0
    };
    let mut bytes = &bytes[first..];
    if let Err(error) = std::str::from_utf8(bytes)
        && error.error_len().is_none()
    {
        bytes = &bytes[..error.valid_up_to()];
    }
    Ok((String::from_utf8_lossy(bytes).into_owned(), start > 0))
}
