//! Incremental reads of the existing saved command output.
use crate::state::Core;
use crate::store;
use serde::Serialize;
use std::io;
use std::io::SeekFrom;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct Output {
    data: Vec<u8>,
    offset: u64,
    done: bool,
}

#[tracing::instrument(target = "zeal::operations", name = "console.read", skip_all)]
pub fn read(data: &Path, core: &Core, id: &str, offset: u64) -> io::Result<Output> {
    use std::io::{Read, Seek};

    let finished = {
        let state = core.state.lock().expect("application state lock poisoned");
        if state.console_run().map(|run| run.as_str()) != Some(id) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Console is no longer available",
            ));
        }
        state
            .console
            .as_ref()
            .expect("Console access was checked")
            .finished
    };
    // The Core read grant authorizes this ID even if Started never associated
    // it with a workspace in history. Process controls use Core.run instead.
    let mut file = store::live_artifact(data, id, "terminal.log")?;
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
        done: finished && offset == length,
    })
}

#[cfg(test)]
mod tests;
