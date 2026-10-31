//! The app-owned loopback Build Event Service. Build receipts acknowledge only
//! after normal EOF, file flush, and the conditional database update.
use crate::protocol::bes as wire;
use crate::state::Core;
use crate::store;
use crate::store::events::{AckRange, Receipt};
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use wire::publish_build_event_server::{PublishBuildEvent, PublishBuildEventServer};

pub const ADDRESS: &str = "127.0.0.1:4718";
const MAX_SESSIONS: usize = 4;
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct Receiver {
    data: PathBuf,
    sessions: Arc<tokio::sync::Semaphore>,
}

impl Receiver {
    pub fn new(data: PathBuf) -> Self {
        Self {
            data,
            sessions: Arc::new(tokio::sync::Semaphore::new(MAX_SESSIONS)),
        }
    }

    #[allow(clippy::result_large_err)]
    fn admit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Status> {
        self.sessions.clone().try_acquire_owned().map_err(|_| {
            Status::resource_exhausted("BES is handling its maximum number of streams")
        })
    }
}

/// The blocking worker owns admission together with its receipt, so cancelling
/// the RPC never permits another worker while this one's disk I/O still runs.
struct Session {
    receipt: Option<Receipt>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

#[allow(clippy::result_large_err)] // The RPC task reports tonic Status by value.
impl Session {
    async fn append(
        mut self,
        data: PathBuf,
        event: wire::OrderedBuildEvent,
    ) -> Result<Self, Status> {
        tokio::task::spawn_blocking(move || {
            if self.receipt.is_none() {
                let stream = event
                    .stream_id
                    .as_ref()
                    .ok_or_else(|| Status::invalid_argument("missing stream id"))?;
                self.receipt = Some(Receipt::begin(&data, stream).map_err(store_status)?);
            }
            let result = self
                .receipt
                .as_mut()
                .expect("receipt was initialized")
                .append(&event);
            if let Err(error) = result {
                if let Some(receipt) = self.receipt.take() {
                    let _ = receipt.partial();
                }
                return Err(store_status(error));
            }
            Ok(self)
        })
        .await
        .map_err(|_| Status::internal("event worker failed"))?
    }

    async fn finish(mut self) -> Result<(Self, AckRange), Status> {
        tokio::task::spawn_blocking(move || {
            let receipt = self.receipt.take().expect("nonempty stream has a receipt");
            let range = receipt.finish().map_err(store_status)?;
            Ok((self, range))
        })
        .await
        .map_err(|_| Status::internal("event worker failed"))?
    }

    async fn partial(mut self) {
        let _ = tokio::task::spawn_blocking(move || {
            let _permit = self._permit;
            if let Some(receipt) = self.receipt.take() {
                let _ = receipt.partial();
            }
        })
        .await;
    }
}

fn store_status(error: store::StoreError) -> Status {
    match error.kind {
        store::ErrorKind::Invalid => Status::invalid_argument(error.to_string()),
        store::ErrorKind::Conflict => Status::failed_precondition(error.to_string()),
        store::ErrorKind::Capacity => Status::resource_exhausted(error.to_string()),
        _ => Status::unavailable(error.to_string()),
    }
}

async fn send_error(
    send: &tokio::sync::mpsc::Sender<Result<wire::PublishBuildToolEventStreamResponse, Status>>,
    error: Status,
) {
    let _ = tokio::time::timeout(IDLE_TIMEOUT, send.send(Err(error))).await;
}

#[tonic::async_trait]
impl PublishBuildEvent for Receiver {
    async fn publish_lifecycle_event(
        &self,
        _request: Request<wire::PublishLifecycleEventRequest>,
    ) -> Result<Response<()>, Status> {
        Ok(Response::new(()))
    }

    type PublishBuildToolEventStreamStream = Pin<
        Box<dyn Stream<Item = Result<wire::PublishBuildToolEventStreamResponse, Status>> + Send>,
    >;

    async fn publish_build_tool_event_stream(
        &self,
        request: Request<tonic::Streaming<wire::PublishBuildToolEventStreamRequest>>,
    ) -> Result<Response<Self::PublishBuildToolEventStreamStream>, Status> {
        let mut session = Session {
            receipt: None,
            _permit: self.admit()?,
        };
        let mut incoming = request.into_inner();
        let data = self.data.clone();
        let (send, receive) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            let mut stream = None;
            loop {
                let request = match tokio::time::timeout(IDLE_TIMEOUT, incoming.message()).await {
                    Ok(Ok(Some(request))) => request,
                    Ok(Ok(None)) => break,
                    result => {
                        session.partial().await;
                        let error = match result {
                            Ok(Err(error)) => error,
                            _ => Status::deadline_exceeded("BES stream was idle"),
                        };
                        send_error(&send, error).await;
                        return;
                    }
                };
                let Some(ordered) = request.ordered_build_event else {
                    session.partial().await;
                    send_error(&send, Status::invalid_argument("missing ordered event")).await;
                    return;
                };
                if stream.is_none() {
                    stream = ordered.stream_id.clone();
                }
                session = match session.append(data.clone(), ordered).await {
                    Ok(session) => session,
                    Err(error) => {
                        send_error(&send, error).await;
                        return;
                    }
                };
            }
            let Some(stream) = stream else {
                return;
            };
            let (_session, range) = match session.finish().await {
                Ok(saved) => saved,
                Err(error) => {
                    send_error(&send, error).await;
                    return;
                }
            };
            for sequence_number in range.first..=range.last {
                let ack = wire::PublishBuildToolEventStreamResponse {
                    stream_id: Some(stream.clone()),
                    sequence_number,
                };
                if !matches!(
                    tokio::time::timeout(IDLE_TIMEOUT, send.send(Ok(ack))).await,
                    Ok(Ok(()))
                ) {
                    return;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(receive))))
    }
}

async fn bind(preferred: &str) -> io::Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind(preferred).await {
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            tokio::net::TcpListener::bind("127.0.0.1:0").await
        }
        result => result,
    }
}

fn unavailable(core: &Core, error: String) {
    let mut state = core.state.lock().expect("application state lock poisoned");
    state.bes_endpoint = None;
    state.bes_error = Some(error);
}

/// Publish only the listener we own. Any server termination revokes readiness.
pub async fn start(data: Result<PathBuf, String>, core: Arc<Core>, changed: impl Fn()) {
    let result: Result<(), String> = async {
        let data = data?;
        let listener = bind(ADDRESS).await.map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let endpoint = format!("grpc://{address}");
        {
            let mut state = core.state.lock().expect("application state lock poisoned");
            state.bes_error = (address.port() != 4718).then(|| {
                format!(
                    "BES port 4718 is occupied; use --bes_backend={endpoint} for terminal builds"
                )
            });
            state.bes_endpoint = Some(endpoint);
        }
        changed();
        serve(listener, data)
            .await
            .map_err(|error| error.to_string())?;
        Err("BES receiver stopped".to_string())
    }
    .await;
    if let Err(error) = result {
        unavailable(&core, error);
        changed();
    }
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    data: PathBuf,
) -> Result<(), tonic::transport::Error> {
    tonic::transport::Server::builder()
        .concurrency_limit_per_connection(MAX_SESSIONS)
        .max_concurrent_streams(MAX_SESSIONS as u32)
        .add_service(
            PublishBuildEventServer::new(Receiver::new(data))
                .max_decoding_message_size(16 * 1024 * 1024),
        )
        .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::bep;
    use prost::Message;
    use std::fs;
    use std::io::Write;
    use std::process::{Command, Stdio};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("zeal-bes-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ordered(id: &str, sequence: i64, event: bep::BuildEvent) -> wire::OrderedBuildEvent {
        wire::OrderedBuildEvent {
            stream_id: Some(wire::StreamId {
                build_id: "build".into(),
                invocation_id: id.into(),
                component: 3,
            }),
            sequence_number: sequence,
            event: Some(wire::BuildEvent {
                bazel_event: Some(prost_types::Any {
                    type_url: "type.googleapis.com/build_event_stream.BuildEvent".into(),
                    value: event.encode_to_vec(),
                }),
                ..Default::default()
            }),
        }
    }

    fn started(id: &str, workspace: &std::path::Path) -> bep::BuildEvent {
        bep::BuildEvent {
            payload: Some(bep::build_event::Payload::Started(bep::BuildStarted {
                uuid: id.into(),
                workspace_directory: workspace.to_string_lossy().into_owned(),
                command: "build".into(),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn lifecycle_is_a_noop_even_when_stream_admission_is_full() {
        let temp = Temp::new();
        let data = temp.0.join("data");
        let receiver = Receiver::new(data.clone());
        let _permits: Vec<_> = (0..MAX_SESSIONS)
            .map(|_| receiver.admit().unwrap())
            .collect();
        assert_eq!(
            receiver.admit().unwrap_err().code(),
            tonic::Code::ResourceExhausted
        );
        receiver
            .publish_lifecycle_event(Request::new(wire::PublishLifecycleEventRequest::default()))
            .await
            .unwrap();
        assert!(!data.exists());
    }

    #[test]
    fn cancelled_append_keeps_admission_until_queued_save_finishes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let temp = Temp::new();
            let receiver = Receiver::new(temp.0.join("data"));
            let (release, wait) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || wait.recv().unwrap());
            let session = Session {
                receipt: None,
                _permit: receiver.admit().unwrap(),
            };
            let id = uuid::Uuid::new_v4().to_string();
            let event = ordered(&id, 1, started(&id, &temp.0));
            let rpc = tokio::spawn(session.append(receiver.data.clone(), event));
            tokio::task::yield_now().await;
            rpc.abort();
            assert!(matches!(rpc.await, Err(error) if error.is_cancelled()));
            assert_eq!(receiver.sessions.available_permits(), MAX_SESSIONS - 1);
            release.send(()).unwrap();
            blocker.await.unwrap();
            let _permits = tokio::time::timeout(
                Duration::from_secs(5),
                receiver.sessions.acquire_many(MAX_SESSIONS as u32),
            )
            .await
            .unwrap()
            .unwrap();
        });
    }

    #[test]
    fn cancelled_partial_keeps_admission_until_queued_save_finishes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let receiver = Receiver::new(PathBuf::new());
            let (release, wait) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || wait.recv().unwrap());
            let session = Session {
                receipt: None,
                _permit: receiver.admit().unwrap(),
            };
            let rpc = tokio::spawn(session.partial());
            tokio::task::yield_now().await;
            rpc.abort();
            assert!(rpc.await.unwrap_err().is_cancelled());
            assert_eq!(receiver.sessions.available_permits(), MAX_SESSIONS - 1);
            release.send(()).unwrap();
            blocker.await.unwrap();
            let _permits = tokio::time::timeout(
                Duration::from_secs(5),
                receiver.sessions.acquire_many(MAX_SESSIONS as u32),
            )
            .await
            .unwrap()
            .unwrap();
        });
    }

    #[tokio::test]
    async fn occupied_preferred_port_binds_an_owned_loopback_listener() {
        let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied = other.local_addr().unwrap();
        let own = bind(&occupied.to_string()).await.unwrap();
        let address = own.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        assert_ne!(address, occupied);
        let connection = tokio::net::TcpStream::connect(address).await.unwrap();
        let (_, accepted) = own.accept().await.unwrap();
        assert_eq!(connection.local_addr().unwrap(), accepted);
    }

    #[tokio::test]
    async fn binding_errors_do_not_publish_an_endpoint() {
        assert!(bind("invalid address").await.is_err());
        let core = Arc::new(Core::new());
        start(Err("app data unavailable".into()), core.clone(), || {}).await;
        let state = core.state.lock().unwrap();
        assert!(state.bes_endpoint.is_none());
        assert_eq!(state.bes_error.as_deref(), Some("app data unavailable"));
    }

    #[test]
    fn server_termination_revokes_the_published_endpoint() {
        let core = Core::new();
        core.state.lock().unwrap().bes_endpoint = Some("grpc://127.0.0.1:12345".into());
        unavailable(&core, "BES receiver stopped".into());
        let state = core.state.lock().unwrap();
        assert!(state.bes_endpoint.is_none());
        assert_eq!(state.bes_error.as_deref(), Some("BES receiver stopped"));
    }

    #[tokio::test]
    async fn grpc_ack_waits_for_eof_and_saved_events() {
        let temp = Temp::new();
        let data = temp.0.join("data");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(serve(listener, data.clone()));
        let mut client = wire::publish_build_event_client::PublishBuildEventClient::connect(
            format!("http://{address}"),
        )
        .await
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        send.send(wire::PublishBuildToolEventStreamRequest {
            ordered_build_event: Some(ordered(&id, 1, started(&id, &temp.0))),
        })
        .await
        .unwrap();
        let mut response = client
            .publish_build_tool_event_stream(ReceiverStream::new(receive))
            .await
            .unwrap()
            .into_inner();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), response.message())
                .await
                .is_err()
        );
        let last = bep::BuildEvent {
            last_message: true,
            ..Default::default()
        };
        send.send(wire::PublishBuildToolEventStreamRequest {
            ordered_build_event: Some(ordered(&id, 2, last)),
        })
        .await
        .unwrap();
        drop(send);
        assert_eq!(
            response.message().await.unwrap().unwrap().sequence_number,
            1
        );
        let record = store::read_record(&data, &temp.0, &id).unwrap();
        assert_eq!(record.cmd.as_deref(), Some("build"));
        assert_eq!(record.exit_code, None);
        assert_eq!(
            response.message().await.unwrap().unwrap().sequence_number,
            2
        );
        assert!(response.message().await.unwrap().is_none());
        let file = store::artifact(&data, &temp.0, &id, "bep.binpb").unwrap();
        assert!(file.metadata().unwrap().len() > 0);
        assert_eq!(store::list(&data, &temp.0).unwrap().len(), 1);
        assert!(!data.join("runs").join(&id).join("log.txt").exists());
        server.abort();
    }

    /// Opt-in test against a locally installed Bazel. Uses a tiny executable
    /// rule with no external language rules or network dependencies.
    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary"]
    async fn real_bazel_records_invocation_and_accepts_standard_input() {
        exercise_real_bazel().await;
    }

    async fn exercise_real_bazel() {
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
        let output_root = temp.0.join("bazel");
        let command = |cmd: &str, target: &str, id: &str| {
            let mut command = Command::new(&bazel);
            command
                .current_dir(&ws)
                .args(["--batch", "--ignore_all_rc_files"])
                .arg(format!("--output_user_root={}", output_root.display()))
                .arg(cmd)
                .args([
                    "--enable_bzlmod=false",
                    "--enable_workspace=true",
                    "--incompatible_autoload_externally=",
                    "--host_platform=//:host",
                    "--platforms=//:host",
                    "--spawn_strategy=local",
                ])
                .arg(format!("--bes_backend=grpc://{address}"))
                .arg("--bes_proxy=")
                .arg("--bes_upload_mode=wait_for_upload_complete")
                .arg("--bes_timeout=10s")
                .arg(format!("--invocation_id={id}"))
                .arg(target);
            command
        };
        for (cmd, target) in [("build", "//:double"), ("test", "//:check")] {
            let id = uuid::Uuid::new_v4().to_string();
            let mut process = command(cmd, target, &id);
            let output = tokio::task::spawn_blocking(move || process.output().unwrap())
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{cmd}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let record = store::read_record(&data, &ws, &id).unwrap();
            assert_eq!(record.cmd.as_deref(), Some(cmd));
            assert_eq!(record.target.as_deref(), Some(target));
            assert_eq!(record.exit_code, Some(0));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let mut command = command("run", "//:double", &id);
        let (send_input, wait_input) = std::sync::mpsc::sync_channel(1);
        let process = tokio::task::spawn_blocking(move || {
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                match wait_input.recv_timeout(Duration::from_millis(25)) {
                    Ok(()) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if child.try_wait().unwrap().is_some() {
                            return child.wait_with_output().unwrap();
                        }
                        assert!(
                            std::time::Instant::now() < deadline,
                            "build completes before input"
                        );
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = child.kill();
                        return child.wait_with_output().unwrap();
                    }
                }
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "program is still waiting for input"
            );
            child.stdin.take().unwrap().write_all(b"21\n").unwrap();
            child.wait_with_output().unwrap()
        });
        tokio::time::timeout(std::time::Duration::from_secs(25), async {
            loop {
                if store::read_record(&data, &ws, &id)
                    .is_ok_and(|build| build.finished_at.is_some())
                    || process.is_finished()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("BEP finishes while the program is waiting");
        let _ = send_input.send(());
        let output = process.await.unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
        let observed = store::read_record(&data, &ws, &id).unwrap();
        assert_eq!(observed.cmd.as_deref(), Some("run"));
        assert_eq!(observed.target.as_deref(), Some("//:double"));
        assert_eq!(observed.exit_code, Some(0));
        assert!(observed.finished_at.is_some());
        assert_eq!(store::list(&data, &ws).unwrap().len(), 3);
        server.abort();
    }
}
