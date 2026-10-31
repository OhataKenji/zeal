//! Read-side views of a saved run: the record plus each observation it left
//! behind (log, build results, timing profile). Every observation is wrapped
//! in [`Section`] so a missing or unreadable artifact never hides the record
//! or another observation.
//!
//! Writing these artifacts is `runner`'s job; this module only reads.

use crate::bep::{self, Bep};
use crate::profile::{self, Profile};
use crate::store::{self, RunRecord};
use serde::Serialize;
use std::io::{self, BufReader};
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct RunDetail {
    pub record: RunRecord,
    pub log: Section<String>,
    pub bep: Section<Bep>,
}

/// Observation availability is independent of the process exit code.
/// Only complete and partial observations carry data.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Section<T> {
    Ok { data: T },
    Incomplete { data: T, note: String },
    Missing,
    ParseError { note: String },
    ReadError { note: String },
}

/// Read the record and available observations from a run dir. A missing or
/// unreadable observation never hides the record or another observation.
#[tracing::instrument(target = "zeal::operations", name = "run.detail", skip_all)]
pub fn run_detail(data: &Path, workspace: &Path, id: &str) -> io::Result<RunDetail> {
    let record = store::read_record(data, workspace, id)?;
    let (log, bep) = match store::event_artifact(data, workspace, id).map_err(io::Error::from) {
        Err(error) => {
            let kind = error.kind();
            let note = error.to_string();
            if kind == io::ErrorKind::NotFound {
                (Section::Missing, Section::Missing)
            } else {
                (
                    Section::ReadError { note: note.clone() },
                    Section::ReadError { note },
                )
            }
        }
        Ok((file, complete)) => {
            let length = file.metadata()?.len();
            let parsed = bep::parse_detail(BufReader::new(file), length);
            if parsed.invalid && parsed.events == 0 {
                let note = "BEP contains an invalid or incomplete event".to_string();
                if parsed.read_error {
                    (
                        Section::ReadError { note: note.clone() },
                        Section::ReadError { note },
                    )
                } else {
                    (
                        Section::ParseError { note: note.clone() },
                        Section::ParseError { note },
                    )
                }
            } else {
                let log = if parsed.invalid || !complete {
                    Section::Incomplete {
                        data: parsed.log,
                        note: "BEP ended with an invalid or incomplete event".into(),
                    }
                } else {
                    Section::Ok { data: parsed.log }
                };
                let bep = if parsed.bep.truncated || !complete {
                    Section::Incomplete {
                        data: parsed.bep,
                        note: "BEP is incomplete or reached its display limit".into(),
                    }
                } else {
                    Section::Ok { data: parsed.bep }
                };
                (log, bep)
            }
        }
    };
    Ok(RunDetail { record, log, bep })
}

/// Read a saved profile, reporting artifact failures as section statuses.
/// Optional artifacts are independent of invocation result and origin.
pub fn run_profile(data: &Path, workspace: &Path, id: &str) -> io::Result<Section<Profile>> {
    store::read_record(data, workspace, id)?;
    Ok({
        match store::artifact(data, workspace, id, "profile.gz") {
            Err(e) => read_error(e),
            Ok(f) => match profile::parse_bounded(f, profile::IPC_LIMIT) {
                Ok(data) => Section::Ok { data },
                Err(profile::ProfileError::Incomplete(note)) => Section::Incomplete {
                    data: Profile {
                        phases: Vec::new(),
                        lanes: Vec::new(),
                    },
                    note,
                },
                Err(profile::ProfileError::Parse(note)) => Section::ParseError { note },
            },
        }
    })
}

fn read_error<T>(error: io::Error) -> Section<T> {
    if error.kind() == io::ErrorKind::NotFound {
        Section::Missing
    } else {
        Section::ReadError {
            note: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::events::test_support::Fixture;
    use std::fs;
    #[test]
    fn event_details_are_shared_and_missing_files_do_not_hide_build_result() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        let detail = run_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(detail.record.exit_code, Some(0));
        assert!(matches!(detail.log, Section::Ok { .. }));
        assert!(matches!(detail.bep, Section::Ok { .. }));
        assert!(matches!(
            run_profile(&f.data, &f.ws, id).unwrap(),
            Section::Missing
        ));
        fs::remove_file(f.dir().join("attempt/1/bep.binpb")).unwrap();
        let missing = run_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(missing.record, detail.record);
        assert!(matches!(missing.log, Section::Missing));
        assert!(matches!(missing.bep, Section::Missing));
    }
    #[test]
    fn malformed_detail_and_optional_profile_are_independent_sections() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        fs::write(f.dir().join("attempt/1/bep.binpb"), b"\xff").unwrap();
        fs::write(f.dir().join("profile.gz"), b"invalid gzip").unwrap();
        let detail = run_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(detail.record.exit_code, Some(0));
        assert!(matches!(detail.log, Section::ParseError { .. }));
        assert!(matches!(detail.bep, Section::ParseError { .. }));
        assert!(matches!(
            run_profile(&f.data, &f.ws, id).unwrap(),
            Section::ParseError { .. }
        ));
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        assert!(run_detail(&f.data, &other, id).is_err());
        assert!(run_profile(&f.data, &other, id).is_err());
    }

    #[test]
    fn valid_prefix_from_a_disconnected_receipt_is_shown_as_incomplete() {
        let f = Fixture::new();
        let mut receipt = store::events::Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        receipt.partial().unwrap();
        let detail = run_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(matches!(detail.log, Section::Incomplete { .. }));
        assert!(matches!(detail.bep, Section::Incomplete { .. }));
        assert_eq!(detail.record.exit_code, None);
    }
}
