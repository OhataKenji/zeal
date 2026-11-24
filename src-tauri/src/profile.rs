//! Bounded reads of the saved Bazel profiling trace. The gzip artifact is
//! handed to the bundled Perfetto viewer verbatim, so only its compressed
//! size is capped here; the expanded JSON is never parsed in Rust.
use std::io::{self, Read};
use std::path::Path;

use crate::store;

/// Compressed artifact bound. The viewer ingests the gzip directly, so the
/// expanded JSON has no Rust-side limit.
pub const COMPRESSED_LIMIT: u64 = 64 * 1024 * 1024;

/// Read the saved trace artifact, refusing anything past the compressed
/// bound. Metadata rejects oversized files before any allocation; the bounded
/// read stays as a backstop if the file grows in between.
pub fn trace(data: &Path, workspace: &Path, id: &str) -> io::Result<Vec<u8>> {
    let file = store::artifact(data, workspace, id, "profile.json.gz")?;
    if file.metadata()?.len() > COMPRESSED_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile exceeds the 64 MiB limit",
        ));
    }
    trace_bytes(file)
}

fn trace_bytes<R: Read>(input: R) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take(COMPRESSED_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > COMPRESSED_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile exceeds the 64 MiB limit",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::events::test_support::Fixture;

    fn fixture_with_profile(bytes: &[u8]) -> (Fixture, String) {
        let f = Fixture::new();
        f.complete();
        let dir = f.dir().join("attempt/1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("profile.json.gz"), bytes).unwrap();
        let id = f.stream.invocation_id.clone();
        (f, id)
    }

    #[test]
    fn trace_returns_the_artifact_bytes_unchanged() {
        let expected: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let (f, id) = fixture_with_profile(&expected);
        assert_eq!(trace(&f.data, &f.ws, &id).unwrap(), expected);
    }

    #[test]
    fn trace_rejects_an_over_cap_artifact() {
        // Sparse file: the read bound trips on length, not allocation.
        let (f, id) = fixture_with_profile(b"gzip");
        let path = f.dir().join("attempt/1/profile.json.gz");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(COMPRESSED_LIMIT + 1).unwrap();
        assert!(trace(&f.data, &f.ws, &id).is_err());
    }

    #[test]
    fn trace_fails_closed_without_an_artifact() {
        let f = Fixture::new();
        f.complete();
        let id = f.stream.invocation_id.clone();
        assert_eq!(
            trace(&f.data, &f.ws, &id).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn trace_bytes_stops_one_past_the_bound() {
        // A file exactly at the limit reads fully; one byte more fails.
        let at_limit = vec![0u8; COMPRESSED_LIMIT as usize];
        let file = std::io::Cursor::new(at_limit.clone());
        assert_eq!(trace_bytes(file).unwrap(), at_limit);
        let over = vec![0u8; COMPRESSED_LIMIT as usize + 1];
        let file = std::io::Cursor::new(over);
        assert!(trace_bytes(file).is_err());
    }
}
