use super::*;
use crate::protocol::bep;
use prost::Message;
use std::fs;

pub(super) struct Temp(pub(super) PathBuf);
impl Temp {
    pub(super) fn new() -> Self {
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
    let mut client = wire::publish_build_event_client::PublishBuildEventClient::connect(format!(
        "http://{address}"
    ))
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
