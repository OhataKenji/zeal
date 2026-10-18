//! Loopback BES. The runner owns sessions; a stream may only save within its
//! captured session. No acknowledgement is emitted before durable commit.
// Tonic RPC signatures use Status directly; boxing would break their trait.
#![allow(clippy::result_large_err)]

use crate::protocol::{bep, bes as wire};
use crate::{state::Core, store};
use prost::Message;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{Request, Response, Status};
use wire::publish_build_event_server::{PublishBuildEvent, PublishBuildEventServer};

const MESSAGE_LIMIT: usize = 8 * 1024 * 1024;
const STREAM_LIMIT: usize = 64 * 1024 * 1024;
const EVENT_LIMIT: usize = 250_000;
const METADATA_LIMIT: usize = 16 * 1024;

// Measure the full payload before prost discards unknown fields. Tonic enforces
// the message bound before calling this decoder. Even nonminimal varints count.
pub struct CountingCodec<T, U>(std::marker::PhantomData<(T, U)>);
impl<T, U> Default for CountingCodec<T, U> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}
pub struct CountingEncoder<T>(std::marker::PhantomData<T>);
pub struct CountingDecoder<U>(std::marker::PhantomData<U>);
impl<T: Message + Send + 'static, U: Message + Default + Send + 'static> tonic::codec::Codec
    for CountingCodec<T, U>
{
    type Encode = T;
    type Decode = U;
    type Encoder = CountingEncoder<T>;
    type Decoder = CountingDecoder<U>;
    fn encoder(&mut self) -> Self::Encoder {
        CountingEncoder(std::marker::PhantomData)
    }
    fn decoder(&mut self) -> Self::Decoder {
        CountingDecoder(std::marker::PhantomData)
    }
}
impl<T: Message> tonic::codec::Encoder for CountingEncoder<T> {
    type Item = T;
    type Error = Status;
    fn encode(&mut self, item: T, buf: &mut tonic::codec::EncodeBuf<'_>) -> Result<(), Status> {
        item.encode(buf)
            .map_err(|_| Status::internal("encode failed"))
    }
}
fn decode_counted<U: Message + Default + 'static>(
    buf: impl prost::bytes::Buf,
) -> Result<U, Status> {
    let size = buf.remaining();
    let mut decoded = U::decode(buf).map_err(|_| Status::invalid_argument("invalid protobuf"))?;
    if let Some(request) = (&mut decoded as &mut dyn std::any::Any).downcast_mut::<WireRequest>() {
        request.wire_size = size;
    }
    Ok(decoded)
}
impl<U: Message + Default + 'static> tonic::codec::Decoder for CountingDecoder<U> {
    type Item = U;
    type Error = Status;
    fn decode(&mut self, buf: &mut tonic::codec::DecodeBuf<'_>) -> Result<Option<U>, Status> {
        decode_counted(buf).map(Some)
    }
}
#[derive(Clone, PartialEq, Message)]
pub struct Envelope {
    #[prost(message, optional, tag = "4")]
    pub ordered_build_event: Option<wire::OrderedBuildEvent>,
}
#[derive(Default, Debug)]
pub struct WireRequest {
    pub inner: Envelope,
    pub wire_size: usize,
}
impl Message for WireRequest {
    fn encode_raw(&self, buf: &mut impl prost::bytes::BufMut) {
        self.inner.encode_raw(buf);
    }
    fn merge_field(
        &mut self,
        tag: u32,
        kind: prost::encoding::WireType,
        buf: &mut impl prost::bytes::Buf,
        ctx: prost::encoding::DecodeContext,
    ) -> Result<(), prost::DecodeError> {
        let before = buf.remaining();
        self.inner.merge_field(tag, kind, buf, ctx)?;
        self.wire_size += before - buf.remaining() + prost::encoding::key_len(tag);
        Ok(())
    }
    fn encoded_len(&self) -> usize {
        self.inner.encoded_len()
    }
    fn clear(&mut self) {
        self.inner.clear();
        self.wire_size = 0;
    }
}

#[derive(Clone)]
pub struct Context {
    pub workspace: PathBuf,
    pub cmd: String,
    pub target: String,
}
#[derive(Clone)]
struct Commit {
    stream: wire::StreamId,
    last: i64,
}
struct SessionState {
    accepting: bool,
    active: bool,
    committing: bool,
    commits: HashMap<String, Commit>,
    error: Option<String>,
}
pub struct Session {
    pub context: Context,
    state: Mutex<SessionState>,
    idle: Condvar,
    cancel: Notify,
    cleanup_failed: Arc<AtomicBool>,
    #[cfg(test)]
    received_bytes: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    received_events: std::sync::atomic::AtomicUsize,
}
impl Session {
    pub fn new(context: Context) -> Arc<Self> {
        Arc::new(Self {
            context,
            state: Mutex::new(SessionState {
                accepting: true,
                active: false,
                committing: false,
                commits: HashMap::new(),
                error: None,
            }),
            idle: Condvar::new(),
            cancel: Notify::new(),
            cleanup_failed: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            received_bytes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            received_events: std::sync::atomic::AtomicUsize::new(0),
        })
    }
    /// Close admission, cancel uncommitted input, and join any started commit.
    pub fn close(&self) -> (Vec<String>, Option<String>) {
        let mut state = self.state.lock().unwrap();
        state.accepting = false;
        self.cancel.notify_one();
        while state.active {
            state = self.idle.wait(state).unwrap();
        }
        let mut error = state.error.clone();
        if self.cleanup_failed.load(Ordering::Acquire) {
            error = Some("BES 一時 file の削除に失敗しました".into());
        }
        (state.commits.keys().cloned().collect(), error)
    }
    #[cfg(test)]
    pub(crate) fn metrics(&self) -> (usize, usize) {
        (
            self.received_bytes.load(Ordering::Relaxed),
            self.received_events.load(Ordering::Relaxed),
        )
    }
    fn accepting(&self) -> bool {
        self.state.lock().unwrap().accepting
    }
}
struct Active(Arc<Session>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().active = false;
        self.0.idle.notify_all();
    }
}

pub struct Server {
    pub port: u16,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }
}
impl Server {
    pub fn start(core: Arc<Core>, data: PathBuf) -> Result<Self, String> {
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(_) => {
                    let _ = ready_tx.send(Err("BES runtime unavailable".to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                    Ok(l) => l,
                    Err(_) => {
                        let _ = ready_tx.send(Err("BES bind failed".to_string()));
                        return;
                    }
                };
                let port = listener.local_addr().unwrap().port();
                let _ = ready_tx.send(Ok(port));
                let service = PublishBuildEventServer::new(Service { core, data })
                    .max_decoding_message_size(MESSAGE_LIMIT)
                    .max_encoding_message_size(MESSAGE_LIMIT);
                let _ = tonic::transport::Server::builder()
                    .http2_max_header_list_size(METADATA_LIMIT as u32)
                    .add_service(service)
                    .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                        let _ = shutdown_rx.await;
                    })
                    .await;
            });
        });
        Ok(Self {
            port: ready_rx.recv().map_err(|_| "BES startup failed")??,
            shutdown: Mutex::new(Some(shutdown_tx)),
        })
    }
}
struct Service {
    core: Arc<Core>,
    data: PathBuf,
}
#[tonic::async_trait]
impl PublishBuildEvent for Service {
    async fn publish_lifecycle_event(
        &self,
        _: Request<wire::PublishLifecycleEventRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented("lifecycle events disabled"))
    }
    type PublishBuildToolEventStreamStream =
        ReceiverStream<Result<wire::PublishBuildToolEventStreamResponse, Status>>;
    async fn publish_build_tool_event_stream(
        &self,
        request: Request<tonic::Streaming<WireRequest>>,
    ) -> Result<Response<Self::PublishBuildToolEventStreamStream>, Status> {
        if request
            .metadata()
            .iter()
            .map(|entry| match entry {
                tonic::metadata::KeyAndValueRef::Ascii(k, v) => {
                    k.as_str().len() + v.as_bytes().len()
                }
                tonic::metadata::KeyAndValueRef::Binary(k, v) => {
                    k.as_str().len() + v.as_encoded_bytes().len()
                }
            })
            .sum::<usize>()
            > METADATA_LIMIT
        {
            return Err(Status::resource_exhausted("metadata limit"));
        }
        let session = self
            .core
            .state
            .lock()
            .unwrap()
            .session
            .clone()
            .ok_or_else(|| Status::unavailable("no active session"))?;
        {
            let mut state = session.state.lock().unwrap();
            if !state.accepting || state.active {
                return Err(Status::unavailable("session unavailable"));
            }
            state.active = true;
        }
        let data = self.data.clone();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            let _active = Active(session.clone());
            let result = receive(&data, &session, request.into_inner(), &tx).await;
            match result {
                Ok((stream, first, last)) => {
                    for sequence_number in first..=last {
                        if !session.accepting() {
                            break;
                        }
                        tokio::select! { _ = session.cancel.notified() => break, result = tx.send(Ok(wire::PublishBuildToolEventStreamResponse { stream_id: Some(stream.clone()), sequence_number })) => if result.is_err() { break; } }
                    }
                }
                Err(status) => {
                    if status.code() == tonic::Code::Internal {
                        session.state.lock().unwrap().error =
                            Some("BES 保存に失敗しました".to_string());
                    }
                    let _ = tx.try_send(Err(status));
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

async fn receive(
    data: &std::path::Path,
    session: &Arc<Session>,
    mut input: tonic::Streaming<WireRequest>,
    response: &mpsc::Sender<Result<wire::PublishBuildToolEventStreamResponse, Status>>,
) -> Result<(wire::StreamId, i64, i64), Status> {
    let mut reducer: Option<Reducer> = None;
    let mut log = None;
    let mut bytes = 0;
    let mut count = 0;
    loop {
        let request = tokio::select! { biased; _ = response.closed() => return Err(Status::cancelled("client disconnected")), _ = session.cancel.notified() => return Err(Status::cancelled("session closed")), message = input.message() => message? };
        if !session.accepting() {
            return Err(Status::cancelled("session closed"));
        }
        let Some(request) = request else {
            break;
        };
        count += 1;
        bytes += request.wire_size;
        #[cfg(test)]
        {
            session
                .received_bytes
                .fetch_add(request.wire_size, Ordering::Relaxed);
            session.received_events.fetch_add(1, Ordering::Relaxed);
        }
        if count > EVENT_LIMIT || bytes > STREAM_LIMIT {
            return Err(Status::resource_exhausted("stream limit"));
        }
        let ordered = request
            .inner
            .ordered_build_event
            .ok_or_else(|| Status::invalid_argument("missing envelope"))?;
        if reducer.is_none() {
            let stream = ordered
                .stream_id
                .as_ref()
                .ok_or_else(|| Status::invalid_argument("missing stream"))?;
            let id = canonical_id(&stream.invocation_id)?;
            let committed = session.state.lock().unwrap().commits.get(&id).cloned();
            reducer = Some(Reducer::new(
                session.context.clone(),
                stream.clone(),
                ordered.sequence_number,
                committed,
            )?);
            if !reducer.as_ref().unwrap().retry() {
                log = Some(
                    store::PreparedLog::new(data)
                        .map_err(|_| Status::internal("log preparation failed"))?,
                );
            }
        }
        if let Some(log) = log.as_mut() {
            log.report_cleanup_to(session.cleanup_failed.clone());
        }
        let chunks = reducer.as_mut().unwrap().push(ordered)?;
        if let Some(log) = log.as_mut() {
            for text in chunks {
                log.append(&text)
                    .map_err(|_| Status::internal("log write failed"))?;
            }
        }
    }
    let reducer = reducer.ok_or_else(|| Status::invalid_argument("empty stream"))?;
    let (record, stream, first, last) = reducer.finish()?;
    if let Some(record) = record {
        {
            let mut state = session.state.lock().unwrap();
            if !state.accepting || response.is_closed() {
                return Err(Status::cancelled("session closed"));
            }
            if state.commits.len() >= 4096 {
                return Err(Status::resource_exhausted("session invocation limit"));
            }
            state.committing = true;
        }
        let data = data.to_path_buf();
        let ws = session.context.workspace.clone();
        // The task is never aborted: close joins active and hence this commit.
        let saved =
            tokio::task::spawn_blocking(move || store::commit(&data, &ws, record, log.unwrap()))
                .await;
        let mut state = session.state.lock().unwrap();
        state.committing = false;
        let saved = saved
            .map_err(|_| Status::internal("commit worker failed"))?
            .map_err(|_| Status::internal("commit failed"))?;
        state.commits.insert(
            saved.id,
            Commit {
                stream: stream.clone(),
                last,
            },
        );
    }
    Ok((stream, first, last))
}
fn canonical_id(id: &str) -> Result<String, Status> {
    uuid::Uuid::parse_str(id)
        .map(|id| id.to_string())
        .map_err(|_| Status::invalid_argument("invalid invocation UUID"))
}
fn invalid() -> Status {
    Status::invalid_argument("invalid build event stream")
}

/// Pure event reducer: validates context once and returns only log fragments.
struct Reducer {
    context: Context,
    stream: wire::StreamId,
    id: String,
    first: i64,
    next: i64,
    committed: Option<Commit>,
    marker: bool,
    started: Option<(u64, String, String)>,
    finished: Option<(Option<u64>, i32)>,
}
impl Reducer {
    fn new(
        context: Context,
        stream: wire::StreamId,
        first: i64,
        committed: Option<Commit>,
    ) -> Result<Self, Status> {
        let id = canonical_id(&stream.invocation_id)?;
        if stream.component != 3 || first < 1 {
            return Err(invalid());
        }
        match &committed {
            Some(c) if c.stream != stream || first > c.last => return Err(invalid()),
            None if first != 1 => return Err(invalid()),
            _ => (),
        }
        Ok(Self {
            context,
            stream,
            id,
            first,
            next: first,
            committed,
            marker: false,
            started: None,
            finished: None,
        })
    }
    fn retry(&self) -> bool {
        self.committed.is_some()
    }
    fn push(&mut self, ordered: wire::OrderedBuildEvent) -> Result<Vec<String>, Status> {
        if self.marker
            || ordered.stream_id.as_ref() != Some(&self.stream)
            || ordered.sequence_number != self.next
        {
            return Err(invalid());
        }
        self.next += 1;
        let event = ordered.event.ok_or_else(invalid)?;
        match event.event {
            Some(wire::build_event::Event::ComponentStreamFinished(marker)) => {
                if marker.r#type != 1 {
                    return Err(invalid());
                }
                self.marker = true;
            }
            Some(wire::build_event::Event::BazelEvent(any)) if !self.retry() => {
                if !any.type_url.ends_with("/build_event_stream.BuildEvent") {
                    return Err(invalid());
                }
                let event = bep::BuildEvent::decode(any.value.as_slice()).map_err(|_| invalid())?;
                match event.payload {
                    Some(bep::build_event::Payload::Started(start)) => {
                        if self.started.is_some()
                            || self.finished.is_some()
                            || canonical_id(&start.uuid)? != self.id
                            || start.command != self.context.cmd
                            || std::path::Path::new(&start.workspace_directory)
                                != self.context.workspace
                        {
                            return Err(invalid());
                        }
                        let patterns: Vec<_> = event
                            .children
                            .iter()
                            .filter_map(|id| id.pattern.as_ref())
                            .filter(|p| !p.pattern.is_empty())
                            .collect();
                        if patterns.len() != 1
                            || patterns[0].pattern != [self.context.target.clone()]
                        {
                            return Err(invalid());
                        }
                        let time = time_ms(start.start_time.as_ref(), start.start_time_millis)
                            .ok_or_else(invalid)?;
                        self.started = Some((time, start.command, patterns[0].pattern[0].clone()));
                    }
                    Some(bep::build_event::Payload::Finished(finish)) => {
                        if self.started.is_none() || self.finished.is_some() {
                            return Err(invalid());
                        }
                        let code = finish.exit_code.ok_or_else(invalid)?.code;
                        self.finished = Some((
                            time_ms(finish.finish_time.as_ref(), finish.finish_time_millis),
                            code,
                        ));
                    }
                    Some(bep::build_event::Payload::Progress(progress)) => {
                        return Ok(vec![progress.stdout, progress.stderr]);
                    }
                    None => (),
                }
            }
            _ => (),
        }
        Ok(Vec::new())
    }
    fn finish(self) -> Result<(Option<store::RunRecord>, wire::StreamId, i64, i64), Status> {
        if !self.marker {
            return Err(invalid());
        }
        let last = self.next - 1;
        if let Some(c) = self.committed {
            if last != c.last {
                return Err(invalid());
            }
            return Ok((None, self.stream, self.first, last));
        }
        let (started_at, cmd, target) = self.started.ok_or_else(invalid)?;
        let (finished_at, code) = self.finished.ok_or_else(invalid)?;
        let record = store::RunRecord {
            id: self.id,
            workspace: self.context.workspace.to_string_lossy().into_owned(),
            workspace_id: String::new(),
            cmd,
            target,
            started_at,
            finished_at,
            exit_code: Some(code),
            bazel_elapsed_ms: finished_at.and_then(|end| end.checked_sub(started_at)),
            outcome: match code {
                0 => "ok",
                8 => "interrupted",
                _ => "failed",
            }
            .into(),
            log_truncated: false,
        };
        Ok((Some(record), self.stream, self.first, last))
    }
}
fn time_ms(timestamp: Option<&prost_types::Timestamp>, legacy: Option<i64>) -> Option<u64> {
    let ms = match timestamp {
        Some(t) if (0..1_000_000_000).contains(&t.nanos) => t
            .seconds
            .checked_mul(1000)?
            .checked_add(i64::from(t.nanos) / 1_000_000)?,
        Some(_) => return None,
        None => legacy?,
    };
    (0..=253_402_300_799_999).contains(&ms).then_some(ms as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use wire::publish_build_event_client::PublishBuildEventClient;
    const ID: &str = "019c9fae-d259-77b2-bd23-e7854f249159";
    fn context(ws: &Path) -> Context {
        Context {
            workspace: ws.into(),
            cmd: "build".into(),
            target: "//...".into(),
        }
    }
    fn stream() -> wire::StreamId {
        wire::StreamId {
            build_id: "build".into(),
            invocation_id: ID.into(),
            component: 3,
        }
    }
    fn event(sequence_number: i64, event: wire::build_event::Event) -> WireRequest {
        WireRequest {
            inner: Envelope {
                ordered_build_event: Some(wire::OrderedBuildEvent {
                    stream_id: Some(stream()),
                    sequence_number,
                    event: Some(wire::BuildEvent { event: Some(event) }),
                }),
            },
            wire_size: 0,
        }
    }
    fn bazel(sequence: i64, event: bep::BuildEvent) -> WireRequest {
        event_any(sequence, event.encode_to_vec())
    }
    fn event_any(sequence: i64, value: Vec<u8>) -> WireRequest {
        event(
            sequence,
            wire::build_event::Event::BazelEvent(prost_types::Any {
                type_url: "type.googleapis.com/build_event_stream.BuildEvent".into(),
                value,
            }),
        )
    }
    fn started(ws: &Path) -> WireRequest {
        bazel(
            1,
            bep::BuildEvent {
                id: None,
                children: vec![bep::BuildEventId {
                    pattern: Some(bep::Pattern {
                        pattern: vec!["//...".into()],
                    }),
                }],
                payload: Some(bep::build_event::Payload::Started(bep::BuildStarted {
                    uuid: ID.into(),
                    start_time_millis: Some(100),
                    command: "build".into(),
                    working_directory: ws.to_string_lossy().into_owned(),
                    workspace_directory: ws.to_string_lossy().into_owned(),
                    start_time: None,
                })),
            },
        )
    }
    fn finished() -> WireRequest {
        bazel(
            3,
            bep::BuildEvent {
                id: None,
                children: vec![],
                payload: Some(bep::build_event::Payload::Finished(bep::BuildFinished {
                    finish_time_millis: Some(150),
                    exit_code: Some(bep::ExitCode { code: 8 }),
                    finish_time: None,
                })),
            },
        )
    }
    fn marker(seq: i64) -> WireRequest {
        event(
            seq,
            wire::build_event::Event::ComponentStreamFinished(wire::ComponentStreamFinished {
                r#type: 1,
            }),
        )
    }
    fn progress() -> WireRequest {
        bazel(
            2,
            bep::BuildEvent {
                id: None,
                children: vec![],
                payload: Some(bep::build_event::Payload::Progress(bep::Progress {
                    stdout: "stdout".into(),
                    stderr: "stderr".into(),
                })),
            },
        )
    }
    fn ordered(request: WireRequest) -> wire::OrderedBuildEvent {
        request.inner.ordered_build_event.unwrap()
    }
    struct Fixture {
        data: PathBuf,
        ws: PathBuf,
        core: Arc<Core>,
        session: Arc<Session>,
        server: Server,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-bes-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let data = root.join("data");
            let ws = root.join("ws");
            std::fs::create_dir(&ws).unwrap();
            let ws = ws.canonicalize().unwrap();
            let core = Arc::new(Core::new());
            let session = Session::new(context(&ws));
            core.state.lock().unwrap().session = Some(session.clone());
            let server = Server::start(core.clone(), data.clone()).unwrap();
            Self {
                data,
                ws,
                core,
                session,
                server,
            }
        }
        async fn client(&self) -> PublishBuildEventClient<tonic::transport::Channel> {
            PublishBuildEventClient::connect(format!("http://127.0.0.1:{}", self.server.port))
                .await
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.session.close();
            let _ = std::fs::remove_dir_all(self.data.parent().unwrap());
        }
    }
    async fn ack(
        mut response: tonic::Streaming<wire::PublishBuildToolEventStreamResponse>,
    ) -> Result<Vec<i64>, Status> {
        let mut numbers = Vec::new();
        while let Some(message) = response.message().await? {
            numbers.push(message.sequence_number);
        }
        Ok(numbers)
    }
    #[test]
    fn wire_byte_count_includes_unknown_fields() {
        let mut bytes = started(Path::new("/ws")).encode_to_vec();
        // Unknown field 100, length-delimited, three bytes payload.
        bytes.extend([0xa2, 0x06, 0x03, 1, 2, 3]);
        let decoded = decode_counted::<WireRequest>(bytes.as_slice()).unwrap();
        assert_eq!(decoded.wire_size, bytes.len());
    }
    #[test]
    fn reducer_uses_bazel_times_exit_and_stdout_then_stderr() {
        let ws = Path::new("/ws");
        let mut reducer = Reducer::new(context(ws), stream(), 1, None).unwrap();
        reducer.push(ordered(started(ws))).unwrap();
        assert_eq!(
            reducer.push(ordered(progress())).unwrap(),
            ["stdout", "stderr"]
        );
        reducer.push(ordered(finished())).unwrap();
        reducer.push(ordered(marker(4))).unwrap();
        let (record, _, _, last) = reducer.finish().unwrap();
        let record = record.unwrap();
        assert_eq!(last, 4);
        assert_eq!(record.outcome, "interrupted");
        assert_eq!(record.bazel_elapsed_ms, Some(50));
    }
    #[test]
    fn reducer_rejects_gaps_mismatched_context_and_missing_final_marker() {
        let ws = Path::new("/ws");
        let mut reducer = Reducer::new(context(ws), stream(), 1, None).unwrap();
        assert!(reducer.push(ordered(started(Path::new("/other")))).is_err());
        let mut reducer = Reducer::new(context(ws), stream(), 1, None).unwrap();
        assert!(reducer.push(ordered(marker(2))).is_err());
        let mut reducer = Reducer::new(context(ws), stream(), 1, None).unwrap();
        reducer.push(ordered(started(ws))).unwrap();
        assert!(reducer.finish().is_err());
    }
    #[test]
    fn timestamp_bounds_and_negative_elapsed_are_not_filled_from_clock() {
        assert_eq!(time_ms(None, Some(0)), Some(0));
        assert_eq!(time_ms(None, Some(-1)), None);
        assert_eq!(time_ms(None, None), None);
        assert_eq!(
            time_ms(
                Some(&prost_types::Timestamp {
                    seconds: 0,
                    nanos: 1_000_000_000
                }),
                Some(100)
            ),
            None
        );
        assert_eq!(time_ms(None, Some(253_402_300_800_000)), None);
    }
    #[tokio::test]
    async fn transport_commits_before_ack_and_accepts_suffix_retry_without_repair() {
        let fixture = Fixture::new();
        let mut client = fixture.client().await;
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![
                started(&fixture.ws),
                progress(),
                finished(),
                marker(4),
            ]))
            .await
            .unwrap();
        assert_eq!(ack(response.into_inner()).await.unwrap(), [1, 2, 3, 4]);
        let records = store::list(&fixture.data, &fixture.ws).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, ID);
        let committed = fixture
            .session
            .state
            .lock()
            .unwrap()
            .commits
            .get(ID)
            .unwrap()
            .clone();
        assert_eq!(committed.last, 4);
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![marker(4)]))
            .await
            .unwrap();
        assert_eq!(ack(response.into_inner()).await.unwrap(), [4]);
        assert_eq!(store::list(&fixture.data, &fixture.ws).unwrap().len(), 1);
    }
    #[tokio::test]
    async fn transport_never_acks_before_eof_and_rejects_concurrent_stream() {
        let fixture = Fixture::new();
        let mut client = fixture.client().await;
        let (tx, rx) = mpsc::channel(4);
        let response = client
            .publish_build_tool_event_stream(ReceiverStream::new(rx))
            .await
            .unwrap();
        let mut response = response.into_inner();
        tx.send(started(&fixture.ws)).await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), response.message())
                .await
                .is_err()
        );
        let error = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![marker(1)]))
            .await
            .err()
            .unwrap();
        assert_eq!(error.code(), tonic::Code::Unavailable);
        drop(tx);
        assert!(ack(response).await.is_err());
        assert!(store::list(&fixture.data, &fixture.ws).unwrap().is_empty());
        assert!(fixture.core.state.lock().unwrap().session.is_some());
    }
    #[tokio::test]
    async fn lifecycle_and_unbound_stream_are_rejected() {
        let fixture = Fixture::new();
        let mut client = fixture.client().await;
        let error = client
            .publish_lifecycle_event(wire::PublishLifecycleEventRequest {})
            .await
            .err()
            .unwrap();
        assert_eq!(error.code(), tonic::Code::Unimplemented);
        fixture.core.state.lock().unwrap().session = None;
        let error = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![marker(1)]))
            .await
            .err()
            .unwrap();
        assert_eq!(error.code(), tonic::Code::Unavailable);
    }
    #[test]
    #[ignore = "requires installed Bazel 8.7.0; runs real local build against loopback BES"]
    fn real_bazel_round_trip() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture.ws.join("MODULE.bazel"),
            "module(name=\"bes_fixture\")\n",
        )
        .unwrap();
        std::fs::write(
            fixture.ws.join("BUILD.bazel"),
            "filegroup(name=\"empty\")\n",
        )
        .unwrap();
        let profile = store::ProfileDir::new(&fixture.data).unwrap();
        let status = std::process::Command::new("bazel")
            .env("USE_BAZEL_VERSION", "8.7.0")
            .arg("--batch")
            .arg("--ignore_all_rc_files")
            .args(crate::runner::argv(
                "build",
                "//...",
                profile.path(),
                fixture.server.port,
            ))
            .current_dir(&fixture.ws)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .status()
            .unwrap();
        assert!(status.success(), "real Bazel did not complete upload");
        let (ids, error) = fixture.session.close();
        assert!(error.is_none(), "{error:?}");
        assert_eq!(ids.len(), 1);
        let records = store::list(&fixture.data, &fixture.ws).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].outcome, "ok");
        profile.cleanup().unwrap();
    }
    #[test]
    fn wire_byte_count_includes_nonminimal_unknown_varints() {
        // Field100 tag encoded with a redundant varint octet; protobuf permits it.
        let bytes = [0xa0, 0x86, 0x00, 0x81, 0x00];
        let decoded = decode_counted::<WireRequest>(bytes.as_slice()).unwrap();
        assert_eq!(decoded.wire_size, bytes.len());
    }
    #[tokio::test]
    async fn complete_retry_never_recreates_deleted_log_and_another_session_cannot_reuse_id() {
        let fixture = Fixture::new();
        let mut client = fixture.client().await;
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![
                started(&fixture.ws),
                progress(),
                finished(),
                marker(4),
            ]))
            .await
            .unwrap();
        assert_eq!(ack(response.into_inner()).await.unwrap(), [1, 2, 3, 4]);
        let record = store::record(&fixture.data, &fixture.ws, ID).unwrap();
        let log = fixture
            .data
            .join("workspaces")
            .join(&record.workspace_id)
            .join("runs")
            .join(ID)
            .join("log.txt");
        std::fs::remove_file(&log).unwrap();
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![
                started(&fixture.ws),
                progress(),
                finished(),
                marker(4),
            ]))
            .await
            .unwrap();
        assert_eq!(ack(response.into_inner()).await.unwrap(), [1, 2, 3, 4]);
        assert!(!log.exists());
        fixture.session.close();
        let next = Session::new(context(&fixture.ws));
        fixture.core.state.lock().unwrap().session = Some(next.clone());
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![
                started(&fixture.ws),
                progress(),
                finished(),
                marker(4),
            ]))
            .await
            .unwrap();
        assert_eq!(
            ack(response.into_inner()).await.unwrap_err().code(),
            tonic::Code::Internal
        );
        next.close();
        assert_eq!(
            store::record(&fixture.data, &fixture.ws, ID).unwrap(),
            record
        );
        assert!(!log.exists());
    }
    #[tokio::test]
    async fn closing_session_cancels_incomplete_input_before_releasing_to_successor() {
        let fixture = Fixture::new();
        let mut client = fixture.client().await;
        let (tx, rx) = mpsc::channel(4);
        let response = client
            .publish_build_tool_event_stream(ReceiverStream::new(rx))
            .await
            .unwrap();
        tx.send(started(&fixture.ws)).await.unwrap();
        let session = fixture.session.clone();
        let closed = tokio::task::spawn_blocking(move || session.close())
            .await
            .unwrap();
        assert!(closed.0.is_empty());
        assert_eq!(
            ack(response.into_inner()).await.unwrap_err().code(),
            tonic::Code::Cancelled
        );
        let next = Session::new(context(&fixture.ws));
        fixture.core.state.lock().unwrap().session = Some(next.clone());
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![
                started(&fixture.ws),
                progress(),
                finished(),
                marker(4),
            ]))
            .await
            .unwrap();
        assert_eq!(ack(response.into_inner()).await.unwrap(), [1, 2, 3, 4]);
        next.close();
        drop(tx);
    }
    #[tokio::test]
    async fn oversized_message_is_rejected_without_record_or_ack() {
        let fixture = Fixture::new();
        let mut client = fixture
            .client()
            .await
            .max_encoding_message_size(MESSAGE_LIMIT * 2);
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![event_any(
                1,
                vec![0; MESSAGE_LIMIT + 1],
            )]))
            .await
            .unwrap();
        assert_eq!(
            ack(response.into_inner()).await.unwrap_err().code(),
            tonic::Code::OutOfRange
        );
        assert!(store::list(&fixture.data, &fixture.ws).unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_bep_payloads_still_exhaust_the_stream_byte_budget() {
        let fixture = Fixture::new();
        let mut client = fixture.client().await;
        let messages = std::iter::once(started(&fixture.ws)).chain((2..12).map(|sequence| {
            let length = 7 * 1024 * 1024;
            let mut bytes = Vec::with_capacity(length + 6);
            bytes.extend([0xa2, 0x06]);
            prost::encoding::encode_varint(length as u64, &mut bytes);
            bytes.resize(bytes.len() + length, 0);
            event_any(sequence, bytes)
        }));
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(messages))
            .await
            .unwrap();
        assert_eq!(
            ack(response.into_inner()).await.unwrap_err().code(),
            tonic::Code::ResourceExhausted
        );
        assert!(fixture.session.metrics().0 > STREAM_LIMIT);
        assert!(store::list(&fixture.data, &fixture.ws).unwrap().is_empty());
    }
    #[tokio::test]
    async fn unknown_envelopes_still_exhaust_the_event_budget() {
        let fixture = Fixture::new();
        let mut client = fixture.client().await;
        let messages = std::iter::once(started(&fixture.ws)).chain(
            (2..=EVENT_LIMIT as i64 + 1).map(|sequence_number| WireRequest {
                inner: Envelope {
                    ordered_build_event: Some(wire::OrderedBuildEvent {
                        stream_id: Some(stream()),
                        sequence_number,
                        event: Some(wire::BuildEvent { event: None }),
                    }),
                },
                wire_size: 0,
            }),
        );
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(messages))
            .await
            .unwrap();
        assert_eq!(
            ack(response.into_inner()).await.unwrap_err().code(),
            tonic::Code::ResourceExhausted
        );
        assert_eq!(fixture.session.metrics().1, EVENT_LIMIT + 1);
        assert!(store::list(&fixture.data, &fixture.ws).unwrap().is_empty());
    }
    #[tokio::test]
    async fn close_joins_started_commit_even_when_ack_response_is_cancelled() {
        let fixture = Fixture::new();
        store::ensure_ready(&fixture.data).unwrap();
        // A real SQLite writer lock pauses the commit without a production hook.
        let blocker = rusqlite::Connection::open(fixture.data.join("zeal.db")).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut client = fixture.client().await;
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![
                started(&fixture.ws),
                progress(),
                finished(),
                marker(4),
            ]))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if fixture.session.state.lock().unwrap().committing {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("stream never reached its commit boundary");
        let (closed_tx, closed_rx) = std::sync::mpsc::sync_channel(1);
        let session = fixture.session.clone();
        let closer = std::thread::spawn(move || {
            closed_tx.send(session.close()).unwrap();
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if !fixture.session.accepting() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("close never closed admission");
        assert!(
            matches!(
                closed_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "close returned before the blocked commit completed"
        );
        drop(response);
        blocker.execute_batch("ROLLBACK").unwrap();
        let (ids, error) = tokio::task::spawn_blocking(move || {
            closed_rx
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
        })
        .await
        .unwrap();
        closer.join().unwrap();
        assert_eq!(ids, [ID]);
        assert_eq!(error, None);
        assert_eq!(
            store::record(&fixture.data, &fixture.ws, ID)
                .unwrap()
                .outcome,
            "interrupted"
        );
        assert_eq!(
            store::read_log(&fixture.data, &fixture.ws, ID)
                .unwrap()
                .text,
            "stdoutstderr"
        );
    }
    #[tokio::test]
    async fn partial_ack_loss_retries_only_the_unacknowledged_suffix() {
        let fixture = Fixture::new();
        let mut client = fixture.client().await;
        let mut response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![
                started(&fixture.ws),
                progress(),
                finished(),
                marker(4),
            ]))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            response.message().await.unwrap().unwrap().sequence_number,
            1
        );
        assert_eq!(
            response.message().await.unwrap().unwrap().sequence_number,
            2
        );
        drop(response);
        let before = store::record(&fixture.data, &fixture.ws, ID).unwrap();
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![finished(), marker(4)]))
            .await
            .unwrap();
        assert_eq!(ack(response.into_inner()).await.unwrap(), [3, 4]);
        assert_eq!(
            store::record(&fixture.data, &fixture.ws, ID).unwrap(),
            before
        );
        assert_eq!(
            store::read_log(&fixture.data, &fixture.ws, ID)
                .unwrap()
                .text,
            "stdoutstderr"
        );
        assert_eq!(store::list(&fixture.data, &fixture.ws).unwrap().len(), 1);
    }
    #[tokio::test]
    async fn app_exit_joins_started_commit_after_process_has_been_reaped() {
        let fixture = Fixture::new();
        store::ensure_ready(&fixture.data).unwrap();
        let blocker = rusqlite::Connection::open(fixture.data.join("zeal.db")).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut client = fixture.client().await;
        let response = client
            .publish_build_tool_event_stream(tokio_stream::iter(vec![
                started(&fixture.ws),
                progress(),
                finished(),
                marker(4),
            ]))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if fixture.session.state.lock().unwrap().committing {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("stream never reached its commit boundary");
        {
            let mut state = fixture.core.state.lock().unwrap();
            state.run = Some("reaped-session".into());
            state.run_pgid = None;
        }
        let (exited_tx, exited_rx) = std::sync::mpsc::sync_channel(1);
        let core = fixture.core.clone();
        let exiting = std::thread::spawn(move || {
            crate::runner::exit_stop(&core);
            exited_tx.send(()).unwrap();
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if !fixture.session.accepting() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("exit did not close session admission after reap");
        assert!(
            matches!(
                exited_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "app exited before its started commit completed"
        );
        drop(response);
        blocker.execute_batch("ROLLBACK").unwrap();
        tokio::task::spawn_blocking(move || {
            exited_rx
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
        })
        .await
        .unwrap();
        exiting.join().unwrap();
        assert_eq!(fixture.session.close().0, [ID]);
        assert_eq!(
            store::record(&fixture.data, &fixture.ws, ID)
                .unwrap()
                .outcome,
            "interrupted"
        );
        assert_eq!(
            store::read_log(&fixture.data, &fixture.ws, ID)
                .unwrap()
                .text,
            "stdoutstderr"
        );
        let state = fixture.core.state.lock().unwrap();
        assert_eq!(state.run_pgid, None);
        assert!(
            !state.run_stopping,
            "a reaped child must never be signalled"
        );
    }
}
