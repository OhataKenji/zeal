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
        let id = uuid::Uuid::new_v4().to_string();
        let dir = store::reserve_invocation(&data, &id).unwrap();
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
    std::fs::write(fixture.dir.join("terminal.log"), vec![0xff; 70 * 1024]).unwrap();
    let first = read(&fixture.data, &fixture.core, &fixture.id, 0).unwrap();
    assert_eq!(first.data.len(), 64 * 1024);
    assert!(!first.done);
    fixture.core.state.lock().unwrap().run = None;
    let middle = read(&fixture.data, &fixture.core, &fixture.id, first.offset).unwrap();
    assert_eq!(middle.data, vec![0xff; 6 * 1024]);
    assert!(!middle.done);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(fixture.dir.join("terminal.log"))
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
fn unobserved_invocation_keeps_its_failure_output_after_capture_finishes() {
    let fixture = Fixture::new();
    std::fs::write(fixture.dir.join("terminal.log"), b"startup failed\n").unwrap();
    {
        let mut state = fixture.core.state.lock().unwrap();
        state.run = None;
        state.console.as_mut().unwrap().finished = true;
    }
    assert!(store::read_record(&fixture.data, &fixture.workspace, &fixture.id).is_err());
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
    let other_id = uuid::Uuid::new_v4().to_string();
    store::reserve_invocation(&fixture.data, &other_id).unwrap();
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
