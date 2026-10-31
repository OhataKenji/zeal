use super::*;
use crate::store;

struct Fixture {
    root: std::path::PathBuf,
    dir: std::path::PathBuf,
    id: String,
    child: Child,
    capture: Capture,
    core: Core,
}
impl Fixture {
    fn new(script: &str) -> Self {
        let root = std::env::temp_dir().join(format!("zeal-pty-{}", uuid::Uuid::new_v4()));
        let ws = root.join("workspace");
        let data = root.join("data");
        std::fs::create_dir_all(&ws).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let dir = store::reserve_invocation(&data, &id).unwrap();
        let (child, control, capture) =
            spawn(Command::new("sh").args(["-c", script]), &dir).unwrap();
        let core = Core::new();
        {
            let mut st = core.state.lock().unwrap();
            st.run = Some(id.clone().into());
            st.run_pgid = Some(child.id() as i32);
            st.run_terminal = Some(control);
        }
        Self {
            root,
            dir,
            id,
            child,
            capture,
            core,
        }
    }
    fn output(&self) -> String {
        std::fs::read_to_string(self.dir.join("terminal.log")).unwrap()
    }
    fn wait_for(&self, text: &str) {
        let until = Instant::now() + Duration::from_secs(5);
        while !self.output().contains(text) {
            assert!(
                Instant::now() < until,
                "waiting for {text}: {}",
                self.output()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn send(&self, data: &[u8]) {
        assert_eq!(write(&self.core, &self.id, data).unwrap(), data.len());
    }
    fn finish(&mut self) -> i32 {
        let code = wait_for_exit(&mut self.child, &self.dir);
        self.capture.finish().unwrap();
        self.core.state.lock().unwrap().run_pgid = None;
        code
    }
}
fn wait_for_exit(child: &mut Child, dir: &Path) -> i32 {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code().unwrap_or(-1);
        }
        assert!(
            Instant::now() < until,
            "child did not exit: {}",
            std::fs::read_to_string(dir.join("terminal.log")).unwrap()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = self.capture.finish();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn unattached_terminal_has_no_foreground_group() {
    let pty = rustix_openpty::openpty(None, None).unwrap();
    let control = Control(File::from(pty.controller));
    assert_eq!(control.foreground_group(), None);
}

#[test]
fn foreground_group_is_cleared_when_session_leader_exits() {
    let mut f = Fixture::new("printf 'ready>'; read value; exit 0");
    f.wait_for("ready>");
    assert_eq!(
        f.core
            .state
            .lock()
            .unwrap()
            .run_terminal
            .as_ref()
            .unwrap()
            .foreground_group(),
        Some(f.child.id() as i32)
    );
    f.send(b"done\r");
    assert_eq!(f.finish(), 0);
    assert_eq!(
        f.core
            .state
            .lock()
            .unwrap()
            .run_terminal
            .as_ref()
            .unwrap()
            .foreground_group(),
        None
    );
}

#[test]
fn interactive_process_has_a_controlling_terminal_and_resizes() {
    let mut f = Fixture::new(
        "test -t 0 && test -t 1 && test -t 2 || exit 9; printf 'ready>'; read value; stty size; printf 'answer=%s\\n' $((value * 2))",
    );
    f.wait_for("ready>");
    assert!(write(&f.core, "old-run", b"999\n").is_err());
    resize(&f.core, &f.id, 120, 40).unwrap();
    f.send(b"21\r");
    assert_eq!(f.finish(), 0);
    assert!(f.output().contains("40 120"));
    assert!(f.output().contains("answer=42"));
    assert!(write(&f.core, &f.id, b"late").is_err());
}

#[test]
fn raw_arrow_keys_do_not_need_enter_and_escape_output_is_saved() {
    let mut f = Fixture::new(
        "stty -echo -icanon min 1 time 0; printf '\\033[2J\\033[Hready'; od -An -t x1 -N 3",
    );
    f.wait_for("ready");
    f.send(b"\x1b[A");
    assert_eq!(f.finish(), 0);
    assert!(
        f.output()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("1b 5b 41"),
        "{}",
        f.output()
    );
    assert!(f.output().contains("\x1b[2J\x1b[H"));
}

#[test]
fn control_c_signals_the_foreground_process() {
    let mut f = Fixture::new("trap 'printf interrupted; exit 130' INT; printf ready; read value");
    f.wait_for("ready");
    f.send(b"\x03");
    assert_eq!(f.finish(), 130);
    assert!(f.output().contains("interrupted"));
}

#[test]
fn control_d_delivers_terminal_eof() {
    let mut f = Fixture::new("printf ready; read value; printf 'eof=%s' $?");
    f.wait_for("ready");
    f.send(b"\x04");
    assert_eq!(f.finish(), 0);
    assert!(f.output().contains("eof=1"));
}
