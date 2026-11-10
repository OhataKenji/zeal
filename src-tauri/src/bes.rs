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
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use wire::publish_build_event_server::{PublishBuildEvent, PublishBuildEventServer};

pub const ADDRESS: &str = "127.0.0.1:4718";
const MAX_STREAMS: usize = 4;
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
type InvocationsChanged = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Receiver {
    data: PathBuf,
    streams: Arc<tokio::sync::Semaphore>,
    invocations_changed: InvocationsChanged,
}

impl Receiver {
    fn new(data: PathBuf, invocations_changed: InvocationsChanged) -> Self {
        Self {
            data,
            streams: Arc::new(tokio::sync::Semaphore::new(MAX_STREAMS)),
            invocations_changed,
        }
    }

    #[allow(clippy::result_large_err)]
    fn admit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Status> {
        self.streams.clone().try_acquire_owned().map_err(|_| {
            Status::resource_exhausted("BES is handling its maximum number of streams")
        })
    }
}

/// The blocking worker owns admission together with its receipt, so cancelling
/// the RPC never permits another worker while this one's disk I/O still runs.
struct Stream {
    receipt: Option<Receipt>,
    _permit: tokio::sync::OwnedSemaphorePermit,
    invocations_changed: InvocationsChanged,
}

#[allow(clippy::result_large_err)] // The RPC task reports tonic Status by value.
impl Stream {
    async fn append(self, data: PathBuf, event: wire::OrderedBuildEvent) -> Result<Self, Status> {
        tokio::task::spawn_blocking(move || self.append_event(&data, &event))
            .await
            .map_err(|_| Status::internal("event worker failed"))?
    }

    fn append_event(
        mut self,
        data: &std::path::Path,
        event: &wire::OrderedBuildEvent,
    ) -> Result<Self, Status> {
        if self.receipt.is_none() {
            let stream = event
                .stream_id
                .as_ref()
                .ok_or_else(|| Status::invalid_argument("missing stream id"))?;
            self.receipt = Some(Receipt::begin(data, stream).map_err(store_status)?);
        }
        let receipt = self.receipt.as_mut().expect("receipt was initialized");
        let changed = match receipt.append(event) {
            Ok(changed) => changed,
            Err(error) => {
                self.save_partial();
                return Err(store_status(error));
            }
        };
        if changed {
            (self.invocations_changed)();
        }
        Ok(self)
    }

    async fn finish(mut self) -> Result<(Self, AckRange), Status> {
        tokio::task::spawn_blocking(move || {
            let receipt = self.receipt.take().expect("nonempty stream has a receipt");
            let range = receipt.finish().map_err(store_status)?;
            (self.invocations_changed)();
            Ok((self, range))
        })
        .await
        .map_err(|_| Status::internal("event worker failed"))?
    }

    async fn partial(self) {
        let _ = tokio::task::spawn_blocking(move || self.save_partial()).await;
    }

    fn save_partial(self) {
        let _permit = self._permit;
        if let Some(receipt) = self.receipt
            && receipt.partial().is_ok()
        {
            (self.invocations_changed)();
        }
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
        Box<
            dyn tokio_stream::Stream<
                    Item = Result<wire::PublishBuildToolEventStreamResponse, Status>,
                > + Send,
        >,
    >;

    async fn publish_build_tool_event_stream(
        &self,
        request: Request<tonic::Streaming<wire::PublishBuildToolEventStreamRequest>>,
    ) -> Result<Response<Self::PublishBuildToolEventStreamStream>, Status> {
        let mut stream = Stream {
            receipt: None,
            _permit: self.admit()?,
            invocations_changed: self.invocations_changed.clone(),
        };
        let mut incoming = request.into_inner();
        let data = self.data.clone();
        let (send, receive) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            let mut stream_id = None;
            loop {
                let request = match tokio::time::timeout(IDLE_TIMEOUT, incoming.message()).await {
                    Ok(Ok(Some(request))) => request,
                    Ok(Ok(None)) => break,
                    result => {
                        stream.partial().await;
                        let error = match result {
                            Ok(Err(error)) => error,
                            _ => Status::deadline_exceeded("BES stream was idle"),
                        };
                        send_error(&send, error).await;
                        return;
                    }
                };
                let Some(ordered) = request.ordered_build_event else {
                    stream.partial().await;
                    send_error(&send, Status::invalid_argument("missing ordered event")).await;
                    return;
                };
                if stream_id.is_none() {
                    stream_id = ordered.stream_id.clone();
                }
                stream = match stream.append(data.clone(), ordered).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        send_error(&send, error).await;
                        return;
                    }
                };
            }
            let Some(stream_id) = stream_id else {
                return;
            };
            let (_stream, range) = match stream.finish().await {
                Ok(saved) => saved,
                Err(error) => {
                    send_error(&send, error).await;
                    return;
                }
            };
            for sequence_number in range.first..=range.last {
                let ack = wire::PublishBuildToolEventStreamResponse {
                    stream_id: Some(stream_id.clone()),
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
pub async fn start(
    data: Result<PathBuf, String>,
    core: Arc<Core>,
    state_changed: impl Fn(),
    invocations_changed: impl Fn() + Send + Sync + 'static,
) {
    let invocations_changed: InvocationsChanged = Arc::new(invocations_changed);
    if let Err(error) = serve_ready(data, &core, &state_changed, invocations_changed).await {
        unavailable(&core, error);
        state_changed();
    }
}

async fn serve_ready(
    data: Result<PathBuf, String>,
    core: &Core,
    state_changed: &impl Fn(),
    invocations_changed: InvocationsChanged,
) -> Result<(), String> {
    let data = data?;
    let listener = bind(ADDRESS).await.map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let endpoint = format!("grpc://{address}");
    {
        let mut state = core.state.lock().expect("application state lock poisoned");
        state.bes_error = (address.port() != 4718).then(|| {
            format!(
                "BES port 4718 is occupied; use --bes_backend={endpoint} for terminal invocations"
            )
        });
        state.bes_endpoint = Some(endpoint);
    }
    state_changed();
    serve_with_invocations_changed(listener, data, invocations_changed)
        .await
        .map_err(|error| error.to_string())?;
    Err("BES receiver stopped".to_string())
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    data: PathBuf,
) -> Result<(), tonic::transport::Error> {
    serve_with_invocations_changed(listener, data, Arc::new(|| {})).await
}

async fn serve_with_invocations_changed(
    listener: tokio::net::TcpListener,
    data: PathBuf,
    invocations_changed: InvocationsChanged,
) -> Result<(), tonic::transport::Error> {
    tonic::transport::Server::builder()
        .concurrency_limit_per_connection(MAX_STREAMS)
        .max_concurrent_streams(MAX_STREAMS as u32)
        .add_service(
            PublishBuildEventServer::new(Receiver::new(data, invocations_changed))
                .max_decoding_message_size(16 * 1024 * 1024),
        )
        .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::bep;
    use crate::store::events::test_support::Fixture;
    use prost::Message;
    use std::fs;
    use std::sync::Mutex;

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

    fn observed_receiver(fixture: &Fixture, seen: Arc<Mutex<Vec<Option<i32>>>>) -> Receiver {
        let data = fixture.data.clone();
        let workspace = fixture.ws.clone();
        let notify = Arc::new(move || {
            let invocations = store::list(&data, &workspace).unwrap();
            seen.lock().unwrap().push(invocations[0].exit_code);
        });
        Receiver::new(fixture.data.clone(), notify)
    }

    fn silent_receiver(data: PathBuf) -> Receiver {
        Receiver::new(data, Arc::new(|| {}))
    }

    #[tokio::test]
    async fn history_notifies_after_started_and_finished_are_committed() {
        let fixture = Fixture::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let receiver = observed_receiver(&fixture, seen.clone());
        let stream = Stream {
            receipt: None,
            _permit: receiver.admit().unwrap(),
            invocations_changed: receiver.invocations_changed.clone(),
        };
        let stream = stream
            .append(fixture.data.clone(), fixture.ordered(1, fixture.started()))
            .await
            .unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![None]);
        let stream = stream
            .append(fixture.data.clone(), fixture.ordered(2, fixture.finished()))
            .await
            .unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![None]);
        stream.finish().await.unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![None, Some(0)]);
    }

    #[tokio::test]
    async fn finish_acknowledges_and_persists_history_for_a_non_git_workspace() {
        let fixture = Fixture::new();
        let receiver = silent_receiver(fixture.data.clone());
        let stream = Stream {
            receipt: None,
            _permit: receiver.admit().unwrap(),
            invocations_changed: receiver.invocations_changed.clone(),
        };
        let stream = stream
            .append(fixture.data.clone(), fixture.ordered(1, fixture.started()))
            .await
            .unwrap();
        let stream = stream
            .append(fixture.data.clone(), fixture.ordered(2, fixture.finished()))
            .await
            .unwrap();
        let (_stream, range) = stream.finish().await.unwrap();
        assert_eq!((range.first, range.last), (1, 2));
        let invocations = store::list(&fixture.data, &fixture.ws).unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].id, fixture.stream.invocation_id);
    }

    #[tokio::test]
    async fn history_notifies_after_partial_save_but_not_invalid_input() {
        let fixture = Fixture::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let receiver = observed_receiver(&fixture, seen.clone());
        let invalid = Stream {
            receipt: None,
            _permit: receiver.admit().unwrap(),
            invocations_changed: receiver.invocations_changed.clone(),
        };
        let mut bad = fixture.ordered(1, fixture.started());
        bad.stream_id = None;
        assert!(invalid.append(fixture.data.clone(), bad).await.is_err());
        assert!(seen.lock().unwrap().is_empty());

        let stream = Stream {
            receipt: None,
            _permit: receiver.admit().unwrap(),
            invocations_changed: receiver.invocations_changed.clone(),
        };
        let stream = stream
            .append(fixture.data.clone(), fixture.ordered(1, fixture.started()))
            .await
            .unwrap();
        let stream = stream
            .append(fixture.data.clone(), fixture.ordered(2, fixture.finished()))
            .await
            .unwrap();
        stream.partial().await;
        assert_eq!(*seen.lock().unwrap(), vec![None, Some(0)]);
    }

    #[tokio::test]
    async fn lifecycle_is_a_noop_even_when_stream_admission_is_full() {
        let temp = Temp::new();
        let data = temp.0.join("data");
        let receiver = silent_receiver(data.clone());
        let _permits: Vec<_> = (0..MAX_STREAMS)
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
    fn cancelled_append_keeps_admission_until_queued_write_finishes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(check_cancelled_append_admission());
    }

    async fn check_cancelled_append_admission() {
        let temp = Temp::new();
        let receiver = silent_receiver(temp.0.join("data"));
        let (release, wait) = std::sync::mpsc::channel();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            wait.recv().unwrap();
        });
        started_rx.await.unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let event = ordered(&id, 1, bep::BuildEvent::default());
        let receipt = Receipt::begin(&receiver.data, event.stream_id.as_ref().unwrap()).unwrap();
        let stream = Stream {
            receipt: Some(receipt),
            _permit: receiver.admit().unwrap(),
            invocations_changed: receiver.invocations_changed.clone(),
        };
        // Queue the blocking write before cancelling the async caller.
        let mut rpc = Box::pin(stream.append(receiver.data.clone(), event));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(rpc.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(rpc);
        assert_eq!(receiver.streams.available_permits(), MAX_STREAMS - 1);
        release.send(()).unwrap();
        blocker.await.unwrap();
        let _permits = tokio::time::timeout(
            Duration::from_secs(5),
            receiver.streams.acquire_many(MAX_STREAMS as u32),
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[test]
    fn cancelled_partial_keeps_admission_until_queued_save_finishes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let receiver = silent_receiver(PathBuf::new());
            let (release, wait) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || wait.recv().unwrap());
            let stream = Stream {
                receipt: None,
                _permit: receiver.admit().unwrap(),
                invocations_changed: receiver.invocations_changed.clone(),
            };
            let rpc = tokio::spawn(stream.partial());
            tokio::task::yield_now().await;
            rpc.abort();
            assert!(rpc.await.unwrap_err().is_cancelled());
            assert_eq!(receiver.streams.available_permits(), MAX_STREAMS - 1);
            release.send(()).unwrap();
            blocker.await.unwrap();
            let _permits = tokio::time::timeout(
                Duration::from_secs(5),
                receiver.streams.acquire_many(MAX_STREAMS as u32),
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
        start(
            Err("app data unavailable".into()),
            core.clone(),
            || (),
            || (),
        )
        .await;
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
}
