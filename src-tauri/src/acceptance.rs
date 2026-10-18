//! Manual acceptance measurements. Synthetic inputs contain no project data.
use crate::bes::{Context, Server, Session};
use crate::{ingest, profile, runner, state::Core, store};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

#[test]
#[ignore = "manual real-Bazel observation overhead and receive/store measurements"]
fn measure_real_bazel() {
    let root = std::env::temp_dir().join(format!("zeal-measure-{}", uuid::Uuid::new_v4()));
    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = ws.canonicalize().unwrap();
    std::fs::write(ws.join("MODULE.bazel"), "module(name=\"zeal_measure\")\n").unwrap();
    let mut build =
        String::from("genrule(name=\"one\", outs=[\"one.txt\"], cmd=\"printf x > $@\")\n");
    for index in 0..64 {
        build.push_str(&format!(
            "genrule(name=\"item{index}\", outs=[\"item{index}.txt\"], cmd=\"printf x > $@\")\n"
        ));
    }
    std::fs::write(ws.join("BUILD.bazel"), build).unwrap();
    let data = root.join("data");
    let core = Arc::new(Core::new());
    let server = Server::start(core.clone(), data.clone()).unwrap();
    // Batch starts a new JVM each time. First sample is cold for this synthetic
    // workspace; repeats reuse disk/action caches. No existing workspace is cleaned.
    for target in ["//:one", "//..."] {
        for mode in ["baseline", "slim", "noslim"] {
            for repetition in 0..3 {
                let session = Session::new(Context {
                    workspace: ws.clone(),
                    cmd: "build".into(),
                    target: target.into(),
                });
                core.state.lock().unwrap().session = Some(session.clone());
                let temporary = store::ProfileDir::new(&data).unwrap();
                let args = if mode == "baseline" {
                    vec!["build".into(), target.into()]
                } else {
                    runner::argv("build", target, temporary.path(), server.port)
                        .into_iter()
                        .filter(|flag| mode != "slim" || flag != "--noslim_profile")
                        .collect()
                };
                let started = Instant::now();
                let status = Command::new("bazel")
                    .env("USE_BAZEL_VERSION", "8.7.0")
                    .args(["--batch", "--ignore_all_rc_files"])
                    .args(args)
                    .current_dir(&ws)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap();
                let process_ms = started.elapsed().as_millis();
                assert!(
                    status.success(),
                    "{target} {mode} sample {repetition} failed"
                );
                let (ids, error) = session.close();
                assert!(error.is_none(), "{error:?}");
                let (received, envelopes) = session.metrics();
                if mode == "baseline" {
                    assert!(ids.is_empty());
                    println!(
                        "BAZEL target={target} mode={mode} sample={repetition} process_ms={process_ms} received_bytes=0"
                    );
                } else {
                    assert_eq!(ids.len(), 1);
                    let id = profile::build_id(temporary.open().unwrap()).unwrap();
                    assert_eq!(ids.as_slice(), std::slice::from_ref(&id));
                    store::attach_profile(&data, &ws, &id, &temporary).unwrap();
                    let compressed = temporary.open().unwrap().metadata().unwrap().len();
                    let expanded = std::io::copy(
                        &mut flate2::read::GzDecoder::new(temporary.open().unwrap()),
                        &mut std::io::sink(),
                    )
                    .unwrap();
                    let started = Instant::now();
                    let detail = ingest::run_detail(&data, &ws, &id).unwrap();
                    assert!(matches!(detail.profile, ingest::Section::Ok { .. }));
                    let parse_ms = started.elapsed().as_micros() as f64 / 1000.0;
                    let ipc = serde_json::to_vec(&detail).unwrap().len();
                    let log = store::read_log(&data, &ws, &id).unwrap().text.len();
                    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
                    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
                    let peak = if cfg!(target_os = "macos") {
                        usage.ru_maxrss as u64
                    } else {
                        usage.ru_maxrss as u64 * 1024
                    };
                    println!(
                        "BAZEL target={target} mode={mode} sample={repetition} process_ms={process_ms} received_bytes={received} envelopes={envelopes} log_bytes={log} compressed_bytes={compressed} expanded_bytes={expanded} parse_ms={parse_ms:.3} ipc_bytes={ipc} harness_peak_rss_bytes={peak}"
                    );
                }
                temporary.cleanup().unwrap();
            }
        }
    }
    core.state.lock().unwrap().session = None;
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}
