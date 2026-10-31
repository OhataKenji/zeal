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

    let (workspace, finished) = {
        let state = core.state.lock().expect("application state lock poisoned");
        if state.console_run().map(|run| run.as_str()) != Some(id) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Console is no longer available",
            ));
        }
        let console = state.console.as_ref().expect("Console access was checked");
        (console.workspace.clone(), console.finished)
    };
    // Read access outlives process control but remains workspace-scoped.
    let mut file = store::artifact(data, &workspace, id, "log.txt")?;
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
mod tests {
    use super::*;
    use crate::state::ConsoleSession;
    use std::io::Write;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        data: PathBuf,
        workspace: PathBuf,
        dir: PathBuf,
        id: String,
        core: Core,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-output-{}", uuid::Uuid::new_v4()));
            let workspace = root.join("workspace");
            let data = root.join("data");
            std::fs::create_dir_all(&workspace).unwrap();
            let (record, dir) = store::create(&data, &workspace, "run", "//:test").unwrap();
            let id = record.id;
            let core = Core::new();
            {
                let mut state = core.state.lock().unwrap();
                state.workspace = Some(workspace.clone());
                state.run = Some(id.clone().into());
                state.console = Some(ConsoleSession {
                    id: id.clone().into(),
                    workspace: workspace.clone(),
                    finished: false,
                });
            }
            Self {
                root,
                data,
                workspace,
                dir,
                id,
                core,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn process_exit_does_not_end_drain_before_the_capture_writes_its_final_bytes() {
        let fixture = Fixture::new();
        std::fs::write(fixture.dir.join("log.txt"), vec![0xff; 70 * 1024]).unwrap();
        let first = read(&fixture.data, &fixture.core, &fixture.id, 0).unwrap();
        assert_eq!(first.data.len(), 64 * 1024);
        assert!(!first.done);
        fixture.core.state.lock().unwrap().run = None;
        let middle = read(&fixture.data, &fixture.core, &fixture.id, first.offset).unwrap();
        assert_eq!(middle.data, vec![0xff; 6 * 1024]);
        assert!(!middle.done);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(fixture.dir.join("log.txt"))
            .unwrap();
        file.write_all(b"program tail\n").unwrap();
        fixture
            .core
            .state
            .lock()
            .unwrap()
            .console
            .as_mut()
            .unwrap()
            .finished = true;
        let last = read(&fixture.data, &fixture.core, &fixture.id, middle.offset).unwrap();
        assert_eq!(last.data, b"program tail\n");
        assert!(last.done);
        let reopened = read(&fixture.data, &fixture.core, &fixture.id, 0).unwrap();
        assert_eq!(reopened.data, first.data);
        assert!(!reopened.done);
        assert!(read(&fixture.data, &fixture.core, &fixture.id, last.offset + 1).is_err());
    }

    #[test]
    fn completed_console_keeps_its_failure_output_after_capture_finishes() {
        let fixture = Fixture::new();
        std::fs::write(fixture.dir.join("log.txt"), b"startup failed\n").unwrap();
        {
            let mut state = fixture.core.state.lock().unwrap();
            state.run = None;
            state.console.as_mut().unwrap().finished = true;
        }
        let output = read(&fixture.data, &fixture.core, &fixture.id, 0).unwrap();
        assert_eq!(output.data, b"startup failed\n");
        assert!(output.done);
        assert_eq!(
            read(&fixture.data, &fixture.core, &fixture.id, 0)
                .unwrap()
                .data,
            output.data
        );
        assert!(crate::terminal::write(&fixture.core, &fixture.id, b"input").is_err());
    }

    #[test]
    fn console_read_grant_rejects_other_ids_workspaces_and_replaced_consoles() {
        let fixture = Fixture::new();
        let (other, _) =
            store::create(&fixture.data, &fixture.workspace, "run", "//:other").unwrap();
        let other_id = other.id;
        assert_eq!(
            read(&fixture.data, &fixture.core, &other_id, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        fixture
            .core
            .state
            .lock()
            .unwrap()
            .console
            .as_mut()
            .unwrap()
            .id = other_id.clone().into();
        assert_eq!(
            read(&fixture.data, &fixture.core, &fixture.id, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(read(&fixture.data, &fixture.core, &other_id, 0).is_ok());
        fixture.core.state.lock().unwrap().workspace = Some(fixture.root.join("other-workspace"));
        assert_eq!(
            read(&fixture.data, &fixture.core, &other_id, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(fixture.core.dto().console_run.is_none());
        {
            let mut state = fixture.core.state.lock().unwrap();
            state.console = None;
            state.workspace = Some(fixture.workspace.clone());
        }
        assert_eq!(
            read(&fixture.data, &fixture.core, &other_id, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
