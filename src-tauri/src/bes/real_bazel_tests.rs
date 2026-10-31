//! Opt-in end-to-end BES coverage using a local Bazel binary.
use super::serve;
use super::tests::Temp;
use crate::store;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Opt-in test against a locally installed Bazel. Uses a tiny executable
/// rule with no external language rules or network dependencies.
#[tokio::test]
#[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary"]
async fn real_bazel_records_invocation_and_accepts_standard_input() {
    let fixture = BazelFixture::new().await;
    fixture.records_invocation("build", "//:double").await;
    fixture.records_invocation("test", "//:check").await;
    fixture.accepts_standard_input().await;
    assert_eq!(store::list(&fixture.data, &fixture.ws).unwrap().len(), 3);
}

struct BazelFixture {
    temp: Temp,
    ws: PathBuf,
    data: PathBuf,
    bazel: String,
    address: std::net::SocketAddr,
    server: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
}

impl Drop for BazelFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl BazelFixture {
    async fn new() -> Self {
        let bazel = std::env::var("ZEAL_TEST_BAZEL").expect("ZEAL_TEST_BAZEL");
        let temp = Temp::new();
        let ws = temp.0.join("workspace");
        let data = temp.0.join("data");
        fs::create_dir(&ws).unwrap();
        fs::write(ws.join("WORKSPACE"), "workspace(name = 'zeal_bes_test')\n").unwrap();
        fs::write(
            ws.join("BUILD"),
            "load(':binary.bzl', 'binary', 'check_test')\nplatform(name='host')\nbinary(name='double')\ncheck_test(name='check')\n",
        )
        .unwrap();
        fs::write(ws.join("binary.bzl"), "def _impl(ctx):\n    out = ctx.actions.declare_file(ctx.label.name + '.py')\n    ctx.actions.write(out, '#!/usr/bin/env python3\\nprint(int(input()) * 2)\\n', is_executable=True)\n    return [DefaultInfo(executable=out)]\nbinary = rule(implementation=_impl, executable=True)\ndef _check(ctx):\n    out=ctx.actions.declare_file(ctx.label.name+'.sh')\n    ctx.actions.write(out,'#!/bin/sh\\nexit 0\\n',is_executable=True)\n    return [DefaultInfo(executable=out)]\ncheck_test=rule(implementation=_check,test=True)\n").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(serve(listener, data.clone()));
        Self {
            temp,
            ws,
            data,
            bazel,
            address,
            server,
        }
    }

    fn command(&self, cmd: &str, target: &str, id: &str) -> Command {
        let mut command = Command::new(&self.bazel);
        command
            .current_dir(&self.ws)
            .args(["--batch", "--ignore_all_rc_files"])
            .arg(format!(
                "--output_user_root={}",
                self.temp.0.join("bazel").display()
            ))
            .arg(cmd)
            .args([
                "--enable_bzlmod=false",
                "--enable_workspace=true",
                "--incompatible_autoload_externally=",
                "--host_platform=//:host",
                "--platforms=//:host",
                "--spawn_strategy=local",
            ])
            .arg(format!("--bes_backend=grpc://{}", self.address))
            .arg("--bes_proxy=")
            .arg("--bes_upload_mode=wait_for_upload_complete")
            .arg("--bes_timeout=10s")
            .arg(format!("--invocation_id={id}"))
            .arg(target);
        command
    }

    async fn records_invocation(&self, cmd: &str, target: &str) {
        let id = uuid::Uuid::new_v4().to_string();
        let mut process = self.command(cmd, target, &id);
        let output = tokio::task::spawn_blocking(move || process.output().unwrap())
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{cmd}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let record = store::read_record(&self.data, &self.ws, &id).unwrap();
        assert_eq!(record.cmd.as_deref(), Some(cmd));
        assert_eq!(record.target.as_deref(), Some(target));
        assert_eq!(record.exit_code, Some(0));
    }

    async fn accepts_standard_input(&self) {
        let id = uuid::Uuid::new_v4().to_string();
        let command = self.command("run", "//:double", &id);
        let (send_input, wait_input) = std::sync::mpsc::sync_channel(1);
        let process =
            tokio::task::spawn_blocking(move || run_with_input_after_build(command, wait_input));
        self.wait_for_build(&id, &process).await;
        let observed = store::read_record(&self.data, &self.ws, &id).unwrap();
        assert!(observed.finished_at.is_some(), "BEP finishes before input");
        let _ = send_input.send(());
        let output = process.await.unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
        let observed = store::read_record(&self.data, &self.ws, &id).unwrap();
        assert_eq!(observed.cmd.as_deref(), Some("run"));
        assert_eq!(observed.target.as_deref(), Some("//:double"));
        assert_eq!(observed.exit_code, Some(0));
        assert!(observed.finished_at.is_some());
    }

    async fn wait_for_build(
        &self,
        id: &str,
        process: &tokio::task::JoinHandle<std::process::Output>,
    ) {
        tokio::time::timeout(
            Duration::from_secs(25),
            wait_for_build_receipt(&self.data, &self.ws, id, process),
        )
        .await
        .expect("BEP finishes while the program is waiting");
    }
}

async fn wait_for_build_receipt(
    data: &std::path::Path,
    workspace: &std::path::Path,
    id: &str,
    process: &tokio::task::JoinHandle<std::process::Output>,
) {
    while !store::read_record(data, workspace, id).is_ok_and(|build| build.finished_at.is_some())
        && !process.is_finished()
    {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// A failed assertion or disconnected input must not leave Bazel waiting.
struct InputProcess(Option<std::process::Child>);

impl InputProcess {
    fn output(mut self) -> std::process::Output {
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for InputProcess {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn run_with_input_after_build(
    mut command: Command,
    wait_input: std::sync::mpsc::Receiver<()>,
) -> std::process::Output {
    let mut process = InputProcess(Some(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let child = process.0.as_mut().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match wait_input.recv_timeout(Duration::from_millis(25)) {
            Ok(()) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                if child.try_wait().unwrap().is_some() =>
            {
                return process.output();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "build completes before input"
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                return process.output();
            }
        }
    }
    assert!(
        child.try_wait().unwrap().is_none(),
        "program is still waiting for input"
    );
    child.stdin.take().unwrap().write_all(b"21\n").unwrap();
    process.output()
}
