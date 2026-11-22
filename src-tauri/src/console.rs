//! Incremental reads of the console session's captured output.
use crate::state::Core;
use serde::Serialize;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

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
    let state = core.state.lock().expect("application state lock poisoned");
    let ws = state
        .workspace_path(ws)
        .map_err(|error| io::Error::new(io::ErrorKind::NotFound, error))?;
    if let Some(invocation) = state.invocations.get(id) {
        if invocation.workspace != ws {
            return Err(unavailable());
        }
        let Some(console) = invocation.console.as_ref() else {
            return Err(unavailable());
        };
        let file = console.try_clone()?;
        drop(state);
        let length = file.metadata()?.len();
        if offset > length {
            return Err(io::Error::other("invalid terminal output offset"));
        }
        let mut bytes = vec![0; (length - offset).min(64 * 1024) as usize];
        file.read_exact_at(&mut bytes, offset)?;
        let offset = offset + bytes.len() as u64;
        return Ok(Output {
            data: bytes,
            offset,
        });
    }
    // A released terminal completion keeps its bounded output tail readable
    // while no live entry or write path remains. Offsets still name the
    // original stream: bytes before the retained window are gone.
    let Some((_, tail)) = state.diagnostics.iter().find(|(diagnostic, _)| {
        diagnostic.id == id && Path::new(&diagnostic.workspace) == ws.as_path()
    }) else {
        return Err(unavailable());
    };
    let end = tail.start + tail.data.len() as u64;
    if offset > end {
        return Err(io::Error::other("invalid terminal output offset"));
    }
    let start = offset.saturating_sub(tail.start) as usize;
    Ok(Output {
        data: tail.data[start..].to_vec(),
        offset: end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ActiveInvocation, DiagnosticTail, OwnedInvocation, ProcessDiagnostic};
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
            state
                .workspaces
                .insert(workspace.clone(), crate::scope::Scope::default());
            state
                .workspaces
                .insert(other.clone(), crate::scope::Scope::default());
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

        /// The worker retained this invocation's diagnostic tail.
        fn retain_diagnostic(&self, start: u64, data: &[u8]) {
            self.core.state.lock().unwrap().diagnostics.push_front((
                ProcessDiagnostic {
                    id: self.id.clone(),
                    workspace: self.ws().to_string(),
                    cmd: "build".to_string(),
                    pattern: "//:b".to_string(),
                    started_at: 1,
                    process_exit_code: Some(9),
                },
                DiagnosticTail {
                    start,
                    data: data.to_vec(),
                },
            ));
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
    fn a_released_diagnostic_serves_its_tail_without_live_state() {
        let fixture = Fixture::new();
        fixture.reap();
        fixture.release();
        // The tail begins at offset 100 of the original stream: offsets keep
        // naming that stream, so a reader polling near the end stays correct.
        let tail = b"Another command is running. Exiting immediately.\r\n";
        let start = 100;
        let end = start + tail.len() as u64;
        fixture.retain_diagnostic(start, tail);
        // An offset before the retained window returns the whole tail.
        let output = fixture.read(0);
        assert_eq!(output.data, tail.to_vec());
        assert_eq!(output.offset, end);
        // An offset inside the window slices from the original position.
        let inside = fixture.read(start + 7);
        assert_eq!(inside.data, tail[7..].to_vec());
        assert_eq!(inside.offset, end);
        // The end is empty; beyond it an error.
        let at_end = fixture.read(end);
        assert!(at_end.data.is_empty());
        assert_eq!(at_end.offset, end);
        assert!(
            read(&fixture.core, fixture.ws(), &fixture.id, end + 1).is_err(),
            "an offset past the retained tail is invalid"
        );
    }

    #[test]
    fn a_released_diagnostic_rejects_foreign_reads_and_writes() {
        let fixture = Fixture::new();
        fixture.reap();
        fixture.release();
        fixture.retain_diagnostic(0, b"tail");
        // Another registered workspace and an unknown id stay rejected.
        assert_eq!(
            read(
                &fixture.core,
                fixture.other.to_str().unwrap(),
                &fixture.id,
                0
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            read(&fixture.core, fixture.ws(), "unknown-id", 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        // Read-only: no write or resize path opens for a released diagnostic.
        assert!(
            crate::terminal::write(&fixture.core, fixture.ws(), &fixture.id, b"input").is_err()
        );
        assert!(crate::terminal::resize(&fixture.core, fixture.ws(), &fixture.id, 80, 24).is_ok());
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
