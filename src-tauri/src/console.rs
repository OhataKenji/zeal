//! Incremental reads of the existing saved command output.
use crate::store;
use serde::Serialize;
use std::io;
use std::io::SeekFrom;
use std::path::Path;

#[derive(Serialize)]
pub struct Output {
    data: Vec<u8>,
    offset: u64,
    done: bool,
}

#[tracing::instrument(target = "zeal::operations", name = "console.read", skip_all)]
pub fn read(
    data: &Path,
    workspace: &Path,
    id: &str,
    offset: u64,
    running: bool,
) -> io::Result<Output> {
    use std::io::{Read, Seek};

    let mut file = store::artifact(data, workspace, id, "log.txt")?;
    let length = file.metadata()?.len();
    if offset > length {
        return Err(io::Error::other("invalid terminal output offset"));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take((length - offset).min(64 * 1024))
        .read_to_end(&mut bytes)?;
    let offset = offset + bytes.len() as u64;
    Ok(Output {
        data: bytes,
        offset,
        done: !running && offset == length,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_is_read_in_bounded_chunks_until_the_finished_file_ends() {
        let root = std::env::temp_dir().join(format!("zeal-output-{}", uuid::Uuid::new_v4()));
        let ws = root.join("workspace");
        let data = root.join("data");
        std::fs::create_dir_all(&ws).unwrap();
        let (record, dir) = store::create(&data, &ws, "build", "//...").unwrap();
        std::fs::write(dir.join("log.txt"), vec![0xff; 70 * 1024]).unwrap();
        let first = read(&data, &ws, &record.id, 0, true).unwrap();
        assert_eq!(first.data.len(), 64 * 1024);
        assert!(!first.done);
        let last = read(&data, &ws, &record.id, first.offset, false).unwrap();
        assert_eq!(last.data, vec![0xff; 6 * 1024]);
        assert!(last.done);
        assert!(read(&data, &ws, &record.id, last.offset + 1, false).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
