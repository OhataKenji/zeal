//! Read-side views of a saved invocation: the record plus each observation it
//! left behind (log, build results, timing profile). Every observation is
//! wrapped
//! in [`Section`] so a missing or unreadable artifact never hides the record
//! or another observation.
//!
//! Writing these artifacts is `runner`'s job; this module only reads.

use crate::bep::{self, Bep};
use crate::profile::{self, Profile};
use crate::store::{self, InvocationRecord};
use serde::Serialize;
use std::fs::File;
use std::io;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct InvocationDetail {
    pub record: InvocationRecord,
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

/// Read the record and available observations from an invocation dir. A
/// missing or unreadable observation never hides the record or another
/// observation.
#[tracing::instrument(target = "zeal::operations", name = "invocation.detail", skip_all)]
pub fn invocation_detail(data: &Path, workspace: &Path, id: &str) -> io::Result<InvocationDetail> {
    let record = store::read_record(data, workspace, id)?;
    let (log, bep) = match store::event_artifact(data, workspace, id).map_err(io::Error::from) {
        Err(error) => {
            if error.kind() == io::ErrorKind::NotFound {
                (Section::Missing, Section::Missing)
            } else {
                event_failure(true, error.to_string())
            }
        }
        Ok((file, complete)) => event_sections(file, complete)?,
    };
    Ok(InvocationDetail { record, log, bep })
}

/// Log and build-result views of the open event artifact. A stream that fails
/// before its first usable event fails both sections alike; afterwards each
/// reports its own completeness.
fn event_sections(file: File, complete: bool) -> io::Result<(Section<String>, Section<Bep>)> {
    let length = file.metadata()?.len();
    let parsed = bep::parse_gzip(file, length);
    if parsed.input_limit_exceeded {
        let note =
            "BEP exceeded its compressed or expanded input limit; showing the accepted prefix";
        return Ok((
            observation(parsed.log, true, note),
            observation(parsed.bep, true, note),
        ));
    }
    if parsed.invalid && parsed.events == 0 {
        let note = "BEP contains an invalid or incomplete event".to_string();
        return Ok(event_failure(parsed.read_error, note));
    }
    let log_incomplete = parsed.invalid || !complete;
    let bep_incomplete = parsed.bep.truncated || !complete;
    Ok((
        observation(
            parsed.log,
            log_incomplete,
            "BEP ended with an invalid or incomplete event",
        ),
        observation(
            parsed.bep,
            bep_incomplete,
            "BEP is incomplete or reached its display limit",
        ),
    ))
}

/// Both sections report the same failure when the event stream is unusable as
/// a whole.
fn event_failure(read_error: bool, note: String) -> (Section<String>, Section<Bep>) {
    if read_error {
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
}

/// A parsed observation keeps its data either way; the status only records
/// whether the stream and the display limit let it finish.
fn observation<T>(data: T, incomplete: bool, note: &str) -> Section<T> {
    if incomplete {
        Section::Incomplete {
            data,
            note: note.into(),
        }
    } else {
        Section::Ok { data }
    }
}

/// Read a saved profile, reporting artifact failures as section statuses.
/// Optional artifacts are independent of invocation result and origin.
pub fn invocation_profile(data: &Path, workspace: &Path, id: &str) -> io::Result<Section<Profile>> {
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
    fn complete_event_shares_log_and_build_result_sections() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        let detail = invocation_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(detail.record.exit_code, Some(0));
        assert!(matches!(detail.log, Section::Ok { .. }));
        assert!(matches!(detail.bep, Section::Ok { .. }));
        assert!(matches!(
            invocation_profile(&f.data, &f.ws, id).unwrap(),
            Section::Missing
        ));
    }

    #[test]
    fn unsafe_gzip_artifact_reports_read_error() {
        use std::os::unix::fs::symlink;

        let f = Fixture::new();
        f.complete();
        let path = f.dir().join("attempt/1/bep.binpb.gz");
        fs::remove_file(&path).unwrap();
        symlink(f.data.join("zeal.db"), path).unwrap();
        let detail = invocation_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(matches!(detail.log, Section::ReadError { .. }));
        assert!(matches!(detail.bep, Section::ReadError { .. }));
    }

    #[test]
    fn live_gzip_never_reports_a_complete_observation() {
        let f = Fixture::new();
        let mut receipt = store::events::Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        let detail = invocation_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(!matches!(detail.log, Section::Ok { .. }));
        assert!(!matches!(detail.bep, Section::Ok { .. }));
        receipt.partial().unwrap();
    }
    #[test]
    fn missing_event_artifact_still_yields_the_record() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        let record = invocation_detail(&f.data, &f.ws, id).unwrap().record;
        fs::remove_file(f.dir().join("attempt/1/bep.binpb.gz")).unwrap();
        let missing = invocation_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(missing.record, record);
        assert!(matches!(missing.log, Section::Missing));
        assert!(matches!(missing.bep, Section::Missing));
    }
    #[test]
    fn malformed_event_artifact_marks_both_sections_parse_error() {
        use std::io::Write;

        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(b"\xff").unwrap();
        fs::write(
            f.dir().join("attempt/1/bep.binpb.gz"),
            gzip.finish().unwrap(),
        )
        .unwrap();
        let detail = invocation_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(detail.record.exit_code, Some(0));
        assert!(matches!(detail.log, Section::ParseError { .. }));
        assert!(matches!(detail.bep, Section::ParseError { .. }));
    }

    #[test]
    fn malformed_gzip_is_a_parse_error() {
        let f = Fixture::new();
        f.complete();
        fs::write(f.dir().join("attempt/1/bep.binpb.gz"), b"invalid gzip").unwrap();
        let detail = invocation_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(matches!(detail.log, Section::ParseError { .. }));
        assert!(matches!(detail.bep, Section::ParseError { .. }));
    }
    #[test]
    fn malformed_profile_reports_its_own_parse_error() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        fs::write(f.dir().join("profile.gz"), b"invalid gzip").unwrap();
        assert!(matches!(
            invocation_profile(&f.data, &f.ws, id).unwrap(),
            Section::ParseError { .. }
        ));
    }
    #[test]
    fn unknown_workspace_fails_detail_and_profile() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        assert!(invocation_detail(&f.data, &other, id).is_err());
        assert!(invocation_profile(&f.data, &other, id).is_err());
    }

    #[test]
    fn valid_prefix_from_a_disconnected_receipt_is_shown_as_incomplete() {
        let f = Fixture::new();
        let mut receipt = store::events::Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        receipt.partial().unwrap();
        let detail = invocation_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(matches!(detail.log, Section::Incomplete { .. }));
        assert!(matches!(detail.bep, Section::Incomplete { .. }));
        assert_eq!(detail.record.exit_code, None);
    }
}
