//! Read-side views of a saved invocation: the record plus each observation it
//! left behind (log, build results). Every observation is wrapped
//! in [`Section`] so a missing or unreadable artifact never hides the record
//! or another observation.
//!
//! The BES receiver writes these artifacts; this module only reads.

use crate::bep::{self, Bep};
use crate::store::{self, InvocationRecord};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read};
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
    Pending,
    ParseError { note: String },
    ReadError { note: String },
}

/// Read the record and available observations from an invocation dir. A
/// missing or unreadable observation never hides the record or another
/// observation.
#[tracing::instrument(target = "zelith::operations", name = "invocation.detail", skip_all)]
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
    // Started can be visible in history before the writer flushes its first events.
    if length == 0 && !complete {
        return Ok((Section::Pending, Section::Pending));
    }
    let parsed = bep::parse_zstd(file, length);
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
            if parsed.invalid {
                "BEP ended with an invalid or incomplete event"
            } else {
                "Build event stream is incomplete"
            },
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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestLogQuery {
    pub label: String,
    #[serde(default)]
    pub configuration: Option<String>,
}

/// Read the zstd-compressed `test.log` saved for one failed test target.
#[tracing::instrument(target = "zelith::operations", name = "invocation.test_log", skip_all)]
pub fn invocation_test_log(
    data: &Path,
    workspace: &Path,
    id: &str,
    query: &TestLogQuery,
) -> io::Result<Section<String>> {
    store::read_record(data, workspace, id)?;
    let name = store::test_log_artifact_name(&query.label, query.configuration.as_deref());
    let file = match store::artifact(data, workspace, id, &name) {
        Err(error) => return Ok(read_error(error)),
        Ok(file) => file,
    };
    let decoder = match zstd::stream::read::Decoder::new(file).and_then(|mut d| {
        d.window_log_max(bep::MAX_WINDOW_LOG)?;
        Ok(d)
    }) {
        Ok(decoder) => decoder,
        Err(error) => {
            return Ok(Section::ParseError {
                note: error.to_string(),
            });
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = decoder
        .take(bep::LOG_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
    {
        return Ok(Section::ParseError {
            note: error.to_string(),
        });
    }
    let truncated = bytes.len() > bep::LOG_LIMIT;
    if truncated {
        bytes.truncate(bep::LOG_LIMIT);
    }
    let data = String::from_utf8_lossy(&bytes).into_owned();
    Ok(if truncated {
        Section::Incomplete {
            data,
            note: "Test log exceeded display limit".into(),
        }
    } else {
        Section::Ok { data }
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
    }

    #[test]
    fn unsafe_zstd_artifact_reports_read_error() {
        use std::os::unix::fs::symlink;

        let f = Fixture::new();
        f.complete();
        let path = f.dir().join("attempt/1/bep.binpb.zst");
        fs::remove_file(&path).unwrap();
        symlink(f.data.join("zelith.db"), path).unwrap();
        let detail = invocation_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(matches!(detail.log, Section::ReadError { .. }));
        assert!(matches!(detail.bep, Section::ReadError { .. }));
    }

    #[test]
    fn buffered_events_are_pending_until_the_receipt_is_flushed() {
        let f = Fixture::new();
        let mut receipt = store::events::Receipt::begin(&f.data, &f.stream).unwrap();
        receipt.append(&f.ordered(1, f.started())).unwrap();
        let detail = invocation_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(matches!(
            (detail.log, detail.bep),
            (Section::Pending, Section::Pending)
        ));
        receipt.partial().unwrap();
        let detail = invocation_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(matches!(
            (detail.log, detail.bep),
            (Section::Incomplete { .. }, Section::Incomplete { .. })
        ));
        assert_eq!(detail.record.exit_code, None);
    }
    #[test]
    fn missing_event_artifact_still_yields_the_record() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        let record = invocation_detail(&f.data, &f.ws, id).unwrap().record;
        fs::remove_file(f.dir().join("attempt/1/bep.binpb.zst")).unwrap();
        let missing = invocation_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(missing.record, record);
        assert!(matches!(missing.log, Section::Missing));
        assert!(matches!(missing.bep, Section::Missing));
    }
    #[test]
    fn malformed_event_artifact_marks_both_sections_parse_error() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        fs::write(
            f.dir().join("attempt/1/bep.binpb.zst"),
            zstd::stream::encode_all(&b"\xff"[..], 0).unwrap(),
        )
        .unwrap();
        let detail = invocation_detail(&f.data, &f.ws, id).unwrap();
        assert_eq!(detail.record.exit_code, Some(0));
        assert!(matches!(detail.log, Section::ParseError { .. }));
        assert!(matches!(detail.bep, Section::ParseError { .. }));
    }

    #[test]
    fn malformed_zstd_is_a_parse_error() {
        let f = Fixture::new();
        f.complete();
        fs::write(f.dir().join("attempt/1/bep.binpb.zst"), b"invalid zstd").unwrap();
        let detail = invocation_detail(&f.data, &f.ws, &f.stream.invocation_id).unwrap();
        assert!(matches!(detail.log, Section::ParseError { .. }));
        assert!(matches!(detail.bep, Section::ParseError { .. }));
    }
    #[test]
    fn unknown_workspace_fails_detail() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        let other = f.root.join("other");
        fs::create_dir(&other).unwrap();
        assert!(invocation_detail(&f.data, &other, id).is_err());
        assert!(
            invocation_test_log(
                &f.data,
                &other,
                id,
                &TestLogQuery {
                    label: "//pkg:t".into(),
                    ..Default::default()
                },
            )
            .is_err()
        );
    }

    #[test]
    fn invocation_test_log_reads_zstd_artifact_and_reports_missing() {
        let f = Fixture::new();
        f.complete();
        let id = &f.stream.invocation_id;
        let name = store::test_log_artifact_name("//pkg:fail_test", None);
        fs::write(
            f.dir().join("attempt/1").join(name),
            zstd::stream::encode_all(b"FAIL: expected 2, got 3\n".as_slice(), 0).unwrap(),
        )
        .unwrap();

        let q = |label: &str| TestLogQuery {
            label: label.into(),
            configuration: None,
        };
        assert!(matches!(
            invocation_test_log(&f.data, &f.ws, id, &q("//pkg:fail_test")).unwrap(),
            Section::Ok { ref data } if data == "FAIL: expected 2, got 3\n"
        ));
        assert!(matches!(
            invocation_test_log(&f.data, &f.ws, id, &q("//pkg:missing")).unwrap(),
            Section::Missing
        ));
    }
}
