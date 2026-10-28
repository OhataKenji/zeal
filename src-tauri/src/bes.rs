//! The app-owned, loopback-only Build Event Service. It never owns child
//! processes. Each stream acknowledges only committed events.
use crate::{protocol::bes as wire, store};
use std::{path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use tokio_stream::{Stream, wrappers::ReceiverStream};
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

    #[allow(clippy::result_large_err)] // tonic's RPC boundary uses Status by value.
    async fn save(
        &self,
        event: wire::OrderedBuildEvent,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<(), Status> {
        let data = self.data.clone();
        tokio::task::spawn_blocking(move || {
            // A cancelled lifecycle RPC must not release admission while its
            // blocking save is still queued or running.
            let _permit = permit;
            store::events::save(&data, &event)
        })
        .await
        .map_err(|_| Status::internal("event worker failed"))?
        .map_err(|error| match error.kind {
            store::ErrorKind::Invalid => Status::invalid_argument(error.to_string()),
            store::ErrorKind::Conflict => {
                Status::failed_precondition("sequence gap or conflicting retry")
            }
            store::ErrorKind::Capacity => Status::resource_exhausted(error.to_string()),
            _ => Status::unavailable(error.to_string()),
        })
    }
}

#[tonic::async_trait]
impl PublishBuildEvent for Receiver {
    async fn publish_lifecycle_event(
        &self,
        request: Request<wire::PublishLifecycleEventRequest>,
    ) -> Result<Response<()>, Status> {
        let session = self.admit()?;
        self.save(
            request
                .into_inner()
                .build_event
                .ok_or_else(|| Status::invalid_argument("missing event"))?,
            Some(session),
        )
        .await?;
        Ok(Response::new(()))
    }

    type PublishBuildToolEventStreamStream = Pin<
        Box<dyn Stream<Item = Result<wire::PublishBuildToolEventStreamResponse, Status>> + Send>,
    >;

    async fn publish_build_tool_event_stream(
        &self,
        request: Request<tonic::Streaming<wire::PublishBuildToolEventStreamRequest>>,
    ) -> Result<Response<Self::PublishBuildToolEventStreamStream>, Status> {
        let session = self.admit()?;
        let mut incoming = request.into_inner();
        let receiver = self.clone();
        let (send, receive) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            let _session = session;
            let mut stream = None;
            loop {
                let result = async {
                    let Some(request) = tokio::time::timeout(IDLE_TIMEOUT, incoming.message())
                        .await
                        .map_err(|_| Status::deadline_exceeded("BES stream was idle"))??
                    else {
                        return Ok(None);
                    };
                    let ordered = request
                        .ordered_build_event
                        .ok_or_else(|| Status::invalid_argument("missing ordered event"))?;
                    let id = ordered
                        .stream_id
                        .clone()
                        .ok_or_else(|| Status::invalid_argument("missing stream id"))?;
                    if stream.as_ref().is_some_and(|previous| previous != &id) {
                        return Err(Status::invalid_argument("stream id changed"));
                    }
                    stream = Some(id.clone());
                    let sequence_number = ordered.sequence_number;
                    receiver.save(ordered, None).await?;
                    Ok(Some(wire::PublishBuildToolEventStreamResponse {
                        stream_id: Some(id),
                        sequence_number,
                    }))
                }
                .await;
                match result {
                    Ok(Some(ack)) => {
                        if !matches!(
                            tokio::time::timeout(IDLE_TIMEOUT, send.send(Ok(ack))).await,
                            Ok(Ok(()))
                        ) {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = tokio::time::timeout(IDLE_TIMEOUT, send.send(Err(error))).await;
                        break;
                    }
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(receive))))
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
    use crate::{protocol::bep, store::events};
    use prost::Message;
    use std::{
        fs,
        io::Write,
        process::{Command, Stdio},
    };

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

    #[tokio::test]
    async fn lifecycle_requests_share_the_stream_admission_limit() {
        let temp = Temp::new();
        let receiver = Receiver::new(temp.0.join("data"));
        let permits: Vec<_> = (0..MAX_SESSIONS)
            .map(|_| receiver.clone().admit().unwrap())
            .collect();
        let error = receiver
            .publish_lifecycle_event(Request::new(wire::PublishLifecycleEventRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::ResourceExhausted);
        drop(permits);
        let error = receiver
            .publish_lifecycle_event(Request::new(wire::PublishLifecycleEventRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn cancelled_lifecycle_keeps_admission_until_queued_save_finishes() {
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
            let request = wire::PublishLifecycleEventRequest {
                build_event: Some(wire::OrderedBuildEvent {
                    stream_id: Some(wire::StreamId {
                        build_id: "build".into(),
                        invocation_id: "invocation".into(),
                        component: 1,
                    }),
                    sequence_number: 1,
                    event: Some(wire::BuildEvent::default()),
                }),
            };
            let worker = receiver.clone();
            let rpc =
                tokio::spawn(
                    async move { worker.publish_lifecycle_event(Request::new(request)).await },
                );
            tokio::task::yield_now().await;
            assert_eq!(receiver.sessions.available_permits(), MAX_SESSIONS - 1);
            rpc.abort();
            assert!(rpc.await.unwrap_err().is_cancelled());
            assert_eq!(receiver.sessions.available_permits(), MAX_SESSIONS - 1);
            release.send(()).unwrap();
            blocker.await.unwrap();
            let permits = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                receiver.sessions.acquire_many(MAX_SESSIONS as u32),
            )
            .await
            .unwrap()
            .unwrap();
            drop(permits);
        });
    }

    #[tokio::test]
    async fn grpc_ack_means_the_event_can_be_read_back() {
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
        let event = bep::BuildEvent {
            payload: Some(bep::build_event::Payload::Started(bep::BuildStarted {
                uuid: id.clone(),
                workspace_directory: temp.0.to_string_lossy().into_owned(),
                command: "build".into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        let ordered = wire::OrderedBuildEvent {
            stream_id: Some(wire::StreamId {
                build_id: "build".into(),
                invocation_id: id.clone(),
                component: 3,
            }),
            sequence_number: 1,
            event: Some(wire::BuildEvent {
                bazel_event: Some(prost_types::Any {
                    type_url: "type.googleapis.com/build_event_stream.BuildEvent".into(),
                    value: event.encode_to_vec(),
                }),
                ..Default::default()
            }),
        };
        let request = wire::PublishBuildToolEventStreamRequest {
            ordered_build_event: Some(ordered),
        };
        let mut response = client
            .publish_build_tool_event_stream(tokio_stream::iter([request.clone(), request]))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            response.message().await.unwrap().unwrap().sequence_number,
            1
        );
        assert_eq!(
            events::get(&data, &temp.0, &id).unwrap().unwrap().cmd,
            "build"
        );
        assert_eq!(
            response.message().await.unwrap().unwrap().sequence_number,
            1
        );
        assert!(response.message().await.unwrap().is_none());
        assert!(store::list(&data, &temp.0).unwrap().is_empty());
        server.abort();
    }

    /// Opt-in test against a locally installed Bazel. Uses a tiny executable
    /// rule with no external language rules or network dependencies.
    #[tokio::test]
    #[ignore = "set ZEAL_TEST_BAZEL to an installed Bazel 8.x binary"]
    async fn real_bazel_records_external_run_and_accepts_standard_input() {
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
            "load(':binary.bzl', 'binary')\nplatform(name='host')\nbinary(name='double')\n",
        )
        .unwrap();
        fs::write(ws.join("binary.bzl"), "def _impl(ctx):\n    out = ctx.actions.declare_file(ctx.label.name + '.py')\n    ctx.actions.write(out, '#!/usr/bin/env python3\\nprint(int(input()) * 2)\\n', is_executable=True)\n    return [DefaultInfo(executable=out)]\nbinary = rule(implementation=_impl, executable=True)\n").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(serve(listener, data.clone()));
        let id = uuid::Uuid::new_v4().to_string();
        let output_root = temp.0.join("bazel");
        let run_ws = ws.clone();
        let run_id = id.clone();
        let (send_input, wait_input) = std::sync::mpsc::sync_channel(1);
        let process = tokio::task::spawn_blocking(move || {
            let mut command = Command::new(bazel);
            command
                .current_dir(run_ws)
                .arg("--batch")
                .arg("--ignore_all_rc_files")
                .arg(format!("--output_user_root={}", output_root.display()))
                .args([
                    "run",
                    "--enable_bzlmod=false",
                    "--enable_workspace=true",
                    "--incompatible_autoload_externally=",
                    "--host_platform=//:host",
                    "--platforms=//:host",
                ])
                .arg(format!("--bes_backend=grpc://{address}"))
                .arg("--bes_timeout=10s")
                .arg(format!("--invocation_id={run_id}"))
                .arg("//:double");
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            wait_input
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("build completes before input");
            assert!(
                child.try_wait().unwrap().is_none(),
                "program is still waiting for input"
            );
            child.stdin.take().unwrap().write_all(b"21\n").unwrap();
            child.wait_with_output().unwrap()
        });
        tokio::time::timeout(std::time::Duration::from_secs(25), async {
            loop {
                if events::get(&data, &ws, &id)
                    .ok()
                    .flatten()
                    .is_some_and(|build| build.complete)
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("BEP finishes while the program is waiting");
        send_input.send(()).unwrap();
        let output = process.await.unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
        let observed = events::get(&data, &ws, &id).unwrap().unwrap();
        assert_eq!(observed.cmd, "run");
        assert_eq!(observed.target, "//:double");
        assert_eq!(observed.exit_code, Some(0));
        assert!(observed.complete);
        assert_eq!(store::list(&data, &ws).unwrap().len(), 1);
        server.abort();
    }
}
