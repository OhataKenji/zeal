//! Incremental reads of the console session's captured output.
use crate::state::Core;
use serde::Serialize;
use std::io;
use std::os::unix::fs::FileExt;

#[derive(Debug, Serialize)]
pub struct Output {
    data: Vec<u8>,
    offset: u64,
}

fn unavailable() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Console is no longer available",
    )
}

#[tracing::instrument(target = "zeal::operations", name = "console.read", skip_all)]
pub fn read(core: &Core, ws: &str, id: &str, offset: u64) -> io::Result<Output> {
    let file = {
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
        console.try_clone()?
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
                    process: ActiveInvocation::stub(Some(42)),
                    console: Some(session_log),
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

        /// The child was reaped; the worker still owns the entry.
        fn reap(&self) {
            self.core
                .state
                .lock()
                .unwrap()
                .invocations
                .get_mut(&self.id)
                .unwrap()
                .process
                .pgid = None;
        }

        /// The worker released the entry: its console is gone with it.
        fn release(&self) {
            self.core.state.lock().unwrap().invocations.remove(&self.id);
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn a_live_console_serves_bounded_chunks_while_draining() {
        let fixture = Fixture::new();
        (&fixture.log).write_all(&vec![0xff; 70 * 1024]).unwrap();
        let first = fixture.read(0);
        assert_eq!(first.data.len(), 64 * 1024);
        fixture.reap();
        let middle = fixture.read(first.offset);
        assert_eq!(middle.data, vec![0xff; 6 * 1024]);
        // Still owned while the capture drains: reads keep working.
        assert_eq!(fixture.read(middle.offset).data.len(), 0);
    }

    #[test]
    fn a_released_console_rejects_reads_and_writes() {
        let fixture = Fixture::new();
        fixture.reap();
        fixture.release();
        assert_eq!(
            read(&fixture.core, fixture.ws(), &fixture.id, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
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
