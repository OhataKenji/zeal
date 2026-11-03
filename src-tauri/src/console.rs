//! Incremental reads of the console session's captured output.
use crate::state::Core;
use serde::Serialize;
use std::io;
use std::os::unix::fs::FileExt;

#[derive(Debug, Serialize)]
pub struct Output {
    data: Vec<u8>,
    offset: u64,
    done: bool,
}

#[tracing::instrument(target = "zeal::operations", name = "console.read", skip_all)]
pub fn read(core: &Core, id: &str, offset: u64) -> io::Result<Output> {
    let (file, finished) = {
        let state = core.state.lock().expect("application state lock poisoned");
        if state.console_invocation().map(|id| id.as_str()) != Some(id) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Console is no longer available",
            ));
        }
        let console = state.console.as_ref().expect("Console access was checked");
        (console.log.try_clone()?, console.finished)
    };
    let length = file.metadata()?.len();
    if offset > length {
        return Err(io::Error::other("invalid terminal output offset"));
    }
    let mut bytes = vec![0; (length - offset).min(64 * 1024) as usize];
    file.read_exact_at(&mut bytes, offset)?;
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
        workspace: PathBuf,
        id: String,
        log: std::fs::File,
        core: Core,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-output-{}", uuid::Uuid::new_v4()));
            let workspace = root.join("workspace");
            std::fs::create_dir_all(&workspace).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let session_log = crate::terminal::session_log().unwrap();
            let log = session_log.try_clone().unwrap();
            let core = Core::new();
            let mut state = core.state.lock().unwrap();
            state.workspace = Some(workspace.clone());
            state.active_invocation = Some(crate::state::ActiveInvocation::stub(&id, None));
            state.console = Some(ConsoleSession {
                id: id.clone().into(),
                workspace: workspace.clone(),
                log: session_log,
                finished: false,
            });
            drop(state);
            Self {
                root,
                workspace,
                id,
                log,
                core,
            }
        }

        fn read(&self, offset: u64) -> Output {
            read(&self.core, &self.id, offset).unwrap()
        }

        /// The process is gone; the console capture may still be draining.
        fn end_invocation(&self) {
            self.core.state.lock().unwrap().active_invocation = None;
        }

        fn finish(&self) {
            self.core
                .state
                .lock()
                .unwrap()
                .console
                .as_mut()
                .unwrap()
                .finished = true;
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn process_exit_does_not_end_drain() {
        let fixture = Fixture::new();
        (&fixture.log).write_all(&vec![0xff; 70 * 1024]).unwrap();
        let first = fixture.read(0);
        assert_eq!(first.data.len(), 64 * 1024);
        assert!(!first.done);
        fixture.end_invocation();
        let middle = fixture.read(first.offset);
        assert_eq!(middle.data, vec![0xff; 6 * 1024]);
        assert!(!middle.done);
    }

    #[test]
    fn a_finished_capture_serves_its_final_bytes_and_replays_the_log() {
        let fixture = Fixture::new();
        (&fixture.log).write_all(&vec![0xff; 70 * 1024]).unwrap();
        fixture.end_invocation();
        (&fixture.log).write_all(b"program tail\n").unwrap();
        fixture.finish();
        let last = fixture.read(70 * 1024);
        assert_eq!(last.data, b"program tail\n");
        assert!(last.done);
        let reopened = fixture.read(0);
        assert_eq!(reopened.data, vec![0xff; 64 * 1024]);
        assert!(!reopened.done);
        assert!(read(&fixture.core, &fixture.id, last.offset + 1).is_err());
    }

    #[test]
    fn unobserved_invocation_keeps_its_failure_output_after_capture_finishes() {
        let fixture = Fixture::new();
        (&fixture.log).write_all(b"startup failed\n").unwrap();
        fixture.end_invocation();
        fixture.finish();
        assert!(
            crate::store::read_record(&fixture.root.join("data"), &fixture.workspace, &fixture.id)
                .is_err()
        );
        let output = fixture.read(0);
        assert_eq!(output.data, b"startup failed\n");
        assert!(output.done);
        assert_eq!(fixture.read(0).data, output.data);
        assert!(crate::terminal::write(&fixture.core, &fixture.id, b"input").is_err());
    }

    #[test]
    fn console_read_grant_rejects_other_ids_workspaces_and_replaced_consoles() {
        let fixture = Fixture::new();
        let other_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            read(&fixture.core, &other_id, 0).unwrap_err().kind(),
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
            read(&fixture.core, &fixture.id, 0).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(read(&fixture.core, &other_id, 0).is_ok());
        fixture.core.state.lock().unwrap().workspace = Some(fixture.root.join("other-workspace"));
        assert_eq!(
            read(&fixture.core, &other_id, 0).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(fixture.core.dto().console_invocation.is_none());
        {
            let mut state = fixture.core.state.lock().unwrap();
            state.console = None;
            state.workspace = Some(fixture.workspace.clone());
        }
        assert_eq!(
            read(&fixture.core, &other_id, 0).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
