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

fn unavailable() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Console is no longer available",
    )
}

#[tracing::instrument(target = "zeal::operations", name = "console.read", skip_all)]
pub fn read(core: &Core, ws: &str, id: &str, offset: u64) -> io::Result<Output> {
    let (file, released) = {
        let state = core.state.lock().expect("application state lock poisoned");
        let ws = state
            .workspace_path(ws)
            .map_err(|error| io::Error::new(io::ErrorKind::NotFound, error))?;
        let Some(invocation) = state.invocations.get(id) else {
            return Err(unavailable());
        };
        if invocation.workspace != ws {
            return Err(unavailable());
        }
        let Some(console) = invocation.console.as_ref() else {
            return Err(unavailable());
        };
        (console.try_clone()?, invocation.process.is_none())
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
        done: released && offset == length,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ActiveInvocation, OwnedInvocation, WorkspaceState};
    use std::io::Write;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        workspace: PathBuf,
        other: PathBuf,
        id: String,
        log: std::fs::File,
        core: Core,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-output-{}", uuid::Uuid::new_v4()));
            let workspace = root.join("workspace");
            let other = root.join("other");
            std::fs::create_dir_all(&workspace).unwrap();
            std::fs::create_dir_all(&other).unwrap();
            let workspace = workspace.canonicalize().unwrap();
            let other = other.canonicalize().unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let session_log = crate::terminal::session_log().unwrap();
            let log = session_log.try_clone().unwrap();
            let core = Core::new();
            let mut state = core.state.lock().unwrap();
            state.workspaces.insert(
                workspace.clone(),
                WorkspaceState::new(crate::scope::Scope::default()),
            );
            state.workspaces.insert(
                other.clone(),
                WorkspaceState::new(crate::scope::Scope::default()),
            );
            state.invocations.insert(
                id.clone(),
                OwnedInvocation {
                    workspace: workspace.clone(),
                    cmd: "build".to_string(),
                    pattern: "//:b".to_string(),
                    started_at: 1,
                    process: Some(ActiveInvocation::stub(Some(42))),
                    console: Some(session_log),
                    process_exit_code: None,
                    note: None,
                },
            );
            drop(state);
            Self {
                root,
                workspace,
                other,
                id,
                log,
                core,
            }
        }

        fn ws(&self) -> &str {
            self.workspace.to_str().unwrap()
        }

        fn read(&self, offset: u64) -> Output {
            read(&self.core, self.ws(), &self.id, offset).unwrap()
        }

        /// The child was reaped; the capture worker still owns the slot.
        fn reap(&self) {
            self.core
                .state
                .lock()
                .unwrap()
                .invocations
                .get_mut(&self.id)
                .unwrap()
                .process
                .as_mut()
                .unwrap()
                .pgid = None;
        }

        /// The worker released the process slot after its final drain.
        fn release(&self) {
            self.core
                .state
                .lock()
                .unwrap()
                .invocations
                .get_mut(&self.id)
                .unwrap()
                .process = None;
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
        fixture.reap();
        let middle = fixture.read(first.offset);
        assert_eq!(middle.data, vec![0xff; 6 * 1024]);
        assert!(!middle.done);
        fixture.release();
        assert!(fixture.read(middle.offset).done);
    }

    #[test]
    fn a_finished_capture_serves_its_final_bytes_and_replays_the_log() {
        let fixture = Fixture::new();
        (&fixture.log).write_all(&vec![0xff; 70 * 1024]).unwrap();
        fixture.reap();
        (&fixture.log).write_all(b"program tail\n").unwrap();
        fixture.release();
        let last = fixture.read(70 * 1024);
        assert_eq!(last.data, b"program tail\n");
        assert!(last.done);
        let reopened = fixture.read(0);
        assert_eq!(reopened.data, vec![0xff; 64 * 1024]);
        assert!(!reopened.done);
        assert!(read(&fixture.core, fixture.ws(), &fixture.id, last.offset + 1).is_err());
    }

    #[test]
    fn unobserved_invocation_keeps_its_failure_output_after_capture_finishes() {
        let fixture = Fixture::new();
        (&fixture.log).write_all(b"startup failed\n").unwrap();
        fixture.reap();
        fixture.release();
        assert!(
            crate::store::read_record(&fixture.root.join("data"), &fixture.workspace, &fixture.id)
                .is_err()
        );
        let output = fixture.read(0);
        assert_eq!(output.data, b"startup failed\n");
        assert!(output.done);
        assert_eq!(fixture.read(0).data, output.data);
        assert!(
            crate::terminal::write(&fixture.core, fixture.ws(), &fixture.id, b"input").is_err()
        );
    }

    #[test]
    fn console_read_requires_the_invocations_own_workspace_and_console() {
        let fixture = Fixture::new();
        let other_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            read(&fixture.core, fixture.ws(), &other_id, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        // The invocation's entry names another registered workspace.
        {
            let mut state = fixture.core.state.lock().unwrap();
            state.invocations.get_mut(&fixture.id).unwrap().workspace = fixture.other.clone();
        }
        assert_eq!(
            read(&fixture.core, fixture.ws(), &fixture.id, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(
            read(
                &fixture.core,
                fixture.other.to_str().unwrap(),
                &fixture.id,
                0
            )
            .is_ok()
        );
        {
            let mut state = fixture.core.state.lock().unwrap();
            let invocation = state.invocations.get_mut(&fixture.id).unwrap();
            invocation.workspace = fixture.workspace.clone();
            invocation.console = None;
        }
        assert_eq!(
            read(&fixture.core, fixture.ws(), &fixture.id, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        let dto = fixture.core.dto();
        assert_eq!(dto.invocations.len(), 1);
        assert!(!dto.invocations[0].console);
    }
}
