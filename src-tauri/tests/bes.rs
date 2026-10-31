//! BES protocol and persistence through the public library API.
use prost::Message;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use zeal_lib::{build_event_protocol as bep, build_event_service_protocol as wire};

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
async fn grpc_ack_waits_for_eof_and_saved_events() {
    let temp = Temp::new();
    let data = temp.0.join("data");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(zeal_lib::serve_build_events(listener, data.clone()));
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
    let record = zeal_lib::read_run_record(&data, &temp.0, &id).unwrap();
    assert_eq!(record.cmd.as_deref(), Some("build"));
    assert_eq!(record.exit_code, None);
    assert_eq!(
        response.message().await.unwrap().unwrap().sequence_number,
        2
    );
    assert!(response.message().await.unwrap().is_none());
    let file = zeal_lib::open_run_artifact(&data, &temp.0, &id, "bep.binpb").unwrap();
    assert!(file.metadata().unwrap().len() > 0);
    assert_eq!(zeal_lib::list_run_records(&data, &temp.0).unwrap().len(), 1);
    assert!(!data.join("runs").join(&id).join("log.txt").exists());
    server.abort();
}

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
