//! Opt-in development traces. No global subscriber or automatic input capture.

use opentelemetry::trace::{Span, TraceContextExt, Tracer, TracerProvider};
use opentelemetry::{Context, KeyValue};
use opentelemetry_otlp::{Protocol, RetryPolicy, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{
    BatchConfigBuilder, BatchSpanProcessor, SdkTracer, SdkTracerProvider,
};
use serde::Deserialize;
use std::time::{Duration, SystemTime};

const EXPORT_TIMEOUT: Duration = Duration::from_millis(750);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub struct Telemetry {
    provider: Option<SdkTracerProvider>,
}

impl Telemetry {
    pub fn from_env() -> Self {
        // Release builds must not even read the endpoint.
        if !cfg!(debug_assertions) {
            return Self::default();
        }
        let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") else {
            return Self::default();
        };
        Self::http(&endpoint).unwrap_or_else(|| {
            // Exporter errors can contain URLs or credentials.
            log::warn!("development tracing disabled: invalid OTLP configuration");
            Self::default()
        })
    }

    fn http(base: &str) -> Option<Self> {
        let endpoint = trace_endpoint(base)?;
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .with_protocol(Protocol::HttpBinary)
            .with_timeout(EXPORT_TIMEOUT)
            .with_retry_policy(RetryPolicy::default().with_max_retries(0))
            .build()
            .ok()?;
        let processor = BatchSpanProcessor::builder(exporter)
            .with_batch_config(
                BatchConfigBuilder::default()
                    .with_max_queue_size(256)
                    .with_max_export_batch_size(32)
                    .with_scheduled_delay(Duration::from_secs(1))
                    .build(),
            )
            .build();
        Some(Self {
            provider: Some(
                SdkTracerProvider::builder()
                    .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
                    .with_span_processor(processor)
                    // Exclude environment resource detectors (paths, user data, etc.).
                    .with_resource(
                        Resource::builder_empty()
                            .with_service_name("zeal-dev")
                            .with_attributes([
                                KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                                KeyValue::new("os.type", std::env::consts::OS),
                                KeyValue::new("zeal.session_id", uuid::Uuid::new_v4().to_string()),
                            ])
                            .build(),
                    )
                    .build(),
            ),
        })
    }

    pub fn enabled(&self) -> bool {
        self.provider.is_some()
    }

    pub fn start(&self, name: &'static str, operation_id: Option<&str>) -> Operation {
        let tracer = self.provider.as_ref().map(|p| p.tracer("zeal"));
        let id = self.enabled().then(|| {
            operation_id
                .and_then(valid_id)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        });
        Operation::start(tracer, name, id, &Context::new())
    }

    pub fn record_ui(&self, record: UiOperation) {
        if !record.duration_ms.is_finite() || !(0.0..=86_400_000.0).contains(&record.duration_ms) {
            return;
        }
        let Some(id) = valid_id(&record.operation_id) else {
            return;
        };
        let Some(provider) = &self.provider else {
            return;
        };
        let tracer = provider.tracer("zeal");
        // Duration is measured with performance.now() in the WebView.
        // Receipt time anchors this separate UI trace; it is not a paint timestamp.
        let start = SystemTime::now() - Duration::from_secs_f64(record.duration_ms / 1000.0);
        let mut span = tracer.build_with_context(
            tracer
                .span_builder(record.command.name())
                .with_start_time(start),
            &Context::new(),
        );
        span.set_attribute(KeyValue::new("zeal.operation_id", id.clone()));
        span.set_attribute(KeyValue::new("zeal.ui.duration_ms", record.duration_ms));
        span.add_event("ui.settled", vec![]);
        let mut operation = Operation {
            tracer: Some(tracer),
            span: Some(span),
            id: Some(id),
            outcome: "unknown",
        };
        operation.outcome(record.outcome.name());
    }

    pub fn shutdown(&self) {
        if let Some(provider) = &self.provider {
            let _ = provider.shutdown_with_timeout(SHUTDOWN_TIMEOUT);
        }
    }
}

fn trace_endpoint(base: &str) -> Option<String> {
    let mut url = url::Url::parse(base).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    url.set_path(&format!("{}/v1/traces", url.path().trim_end_matches('/')));
    Some(url.into())
}

fn valid_id(id: &str) -> Option<String> {
    uuid::Uuid::parse_str(id)
        .ok()
        .filter(|id| !id.is_nil())
        .map(|id| id.to_string())
}

#[derive(Default)]
pub struct Operation {
    tracer: Option<SdkTracer>,
    span: Option<opentelemetry_sdk::trace::Span>,
    pub id: Option<String>,
    outcome: &'static str,
}

impl Operation {
    pub fn result<T, E>(&mut self, result: Result<T, E>) -> Result<T, E> {
        self.outcome(if result.is_ok() { "ok" } else { "zeal_error" });
        result
    }
    fn start(
        tracer: Option<SdkTracer>,
        name: &'static str,
        id: Option<String>,
        parent: &Context,
    ) -> Self {
        let span = tracer.as_ref().map(|tracer| {
            let mut span = tracer.start_with_context(name, parent);
            if let Some(id) = &id {
                span.set_attribute(KeyValue::new("zeal.operation_id", id.clone()));
            }
            span.set_attribute(KeyValue::new("zeal.outcome", "unknown"));
            span
        });
        Self {
            tracer,
            span,
            id,
            outcome: "unknown",
        }
    }

    pub fn child(&self, name: &'static str) -> Self {
        let parent = self.span.as_ref().map_or_else(Context::new, |span| {
            Context::new().with_remote_span_context(span.span_context().clone())
        });
        Self::start(self.tracer.clone(), name, self.id.clone(), &parent)
    }

    pub fn outcome(&mut self, outcome: &'static str) {
        self.outcome = outcome;
        self.field("zeal.outcome", outcome);
    }

    pub fn field(&mut self, name: &'static str, value: &'static str) {
        if let Some(span) = &mut self.span {
            span.set_attribute(KeyValue::new(name, value));
        }
    }

    pub fn count(&mut self, name: &'static str, value: usize) {
        if let Some(span) = &mut self.span {
            span.set_attribute(KeyValue::new(name, value.min(i64::MAX as usize) as i64));
        }
    }

    pub fn run_id(&mut self, id: &str) {
        if let Some(id) = valid_id(id)
            && let Some(span) = &mut self.span
        {
            span.set_attribute(KeyValue::new("zeal.run_id", id));
        }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if let Some(span) = &mut self.span {
            span.add_event(self.outcome, vec![]);
            span.end();
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiCommand {
    OpenWorkspace,
    Refresh,
    StartRun,
    StopRun,
    ListRuns,
    GetRun,
    StateApplied,
}

impl UiCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::OpenWorkspace => "ui.open_workspace",
            Self::Refresh => "ui.refresh",
            Self::StartRun => "ui.start_run",
            Self::StopRun => "ui.stop_run",
            Self::ListRuns => "ui.list_runs",
            Self::GetRun => "ui.get_run",
            Self::StateApplied => "ui.state_applied",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiOutcome {
    Applied,
    Discarded,
    Failed,
}

impl UiOutcome {
    fn name(&self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Discarded => "discarded",
            Self::Failed => "failed",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiOperation {
    operation_id: String,
    command: UiCommand,
    duration_ms: f64,
    outcome: UiOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::trace::InMemorySpanExporter;
    use std::sync::mpsc;
    use std::thread;

    fn memory() -> (Telemetry, InMemorySpanExporter) {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_resource(
                Resource::builder_empty()
                    .with_service_name("zeal-test")
                    .build(),
            )
            .with_simple_exporter(exporter.clone())
            .build();
        (
            Telemetry {
                provider: Some(provider),
            },
            exporter,
        )
    }

    #[test]
    fn endpoint_is_an_explicit_http_base_url() {
        assert_eq!(
            trace_endpoint("http://127.0.0.1:4318"),
            Some("http://127.0.0.1:4318/v1/traces".into())
        );
        assert_eq!(
            trace_endpoint("https://example.org/otel/"),
            Some("https://example.org/otel/v1/traces".into())
        );
        assert!(trace_endpoint("").is_none());
        assert!(trace_endpoint("file:///tmp/trace").is_none());
        assert!(trace_endpoint("http://user:secret@localhost:4318").is_none());
        assert!(trace_endpoint("http://localhost:4318?secret=value").is_none());
        assert!(trace_endpoint("http://localhost:4318/#fragment").is_none());
    }

    #[test]
    fn disabled_operations_have_no_ids_or_sdk_state() {
        let telemetry = Telemetry::default();
        let parent = telemetry.start("run", Some("d7374bd0-56aa-4c21-9f7a-791bbab8fd90"));
        assert!(!telemetry.enabled());
        assert!(parent.id.is_none());
        assert!(parent.child("bazel.wait").span.is_none());
        telemetry.shutdown();
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn release_ignores_endpoint() {
        let telemetry = Telemetry::from_env();
        assert!(!telemetry.enabled());
        assert!(telemetry.start("run", None).span.is_none());
    }

    #[test]
    fn worker_keeps_the_operation_open_and_preserves_its_parent() {
        let (telemetry, exporter) = memory();
        let operation = telemetry.start("run", None);
        let id = operation.id.clone().unwrap();
        let child = operation.child("bazel.wait");
        let (release, released) = mpsc::channel();
        let worker = thread::spawn(move || {
            released.recv().unwrap();
            let mut child = child;
            child.outcome("ok");
            drop(child);
            let mut operation = operation;
            operation.outcome("ok");
        });
        assert!(exporter.get_finished_spans().unwrap().is_empty());
        release.send(()).unwrap();
        worker.join().unwrap();
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 2);
        let parent = spans.iter().find(|s| s.name == "run").unwrap();
        let child = spans.iter().find(|s| s.name == "bazel.wait").unwrap();
        assert_eq!(child.parent_span_id, parent.span_context.span_id());
        assert_eq!(
            child.span_context.trace_id(),
            parent.span_context.trace_id()
        );
        assert_eq!(
            parent
                .attributes
                .iter()
                .find(|a| a.key.as_str() == "zeal.operation_id")
                .unwrap()
                .value
                .as_str(),
            id
        );
        assert!(parent.end_time >= child.end_time);
    }

    #[test]
    fn only_the_final_outcome_becomes_an_event() {
        let (telemetry, exporter) = memory();
        let mut operation = telemetry.start("run", None);
        operation.outcome("rejected");
        operation.outcome("zeal_error");
        operation.outcome("unknown");
        operation.outcome("ok");
        drop(operation);
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans[0].events.events.len(), 1);
        assert_eq!(spans[0].events.events[0].name, "ok");
    }

    #[test]
    fn ui_records_and_errors_cannot_export_free_text() {
        let (telemetry, exporter) = memory();
        let secret = "/private/workspace/secret-token";
        let mut operation = telemetry.start("run.detail", Some(secret));
        let result: Result<(), _> = operation.result(Err(secret));
        assert!(result.is_err());
        operation.run_id(secret);
        drop(operation);
        telemetry.record_ui(UiOperation {
            command: UiCommand::GetRun,
            operation_id: secret.into(),
            duration_ms: 1.0,
            outcome: UiOutcome::Failed,
        });
        telemetry.record_ui(UiOperation {
            command: UiCommand::GetRun,
            operation_id: uuid::Uuid::new_v4().to_string(),
            duration_ms: f64::INFINITY,
            outcome: UiOutcome::Failed,
        });
        assert!(
            serde_json::from_value::<UiOperation>(serde_json::json!({
                "command": "get_run", "operationId": uuid::Uuid::new_v4().to_string(),
                "durationMs": 1, "outcome": "applied", "target": secret
            }))
            .is_err()
        );
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        assert!(!format!("{spans:?}").contains(secret));
    }

    #[test]
    fn ui_duration_and_outcome_are_recorded_once() {
        let (telemetry, exporter) = memory();
        let id = uuid::Uuid::new_v4().to_string();
        telemetry.record_ui(UiOperation {
            command: UiCommand::GetRun,
            operation_id: id.clone(),
            duration_ms: 125.0,
            outcome: UiOutcome::Discarded,
        });
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "ui.get_run");
        assert!(
            spans[0]
                .end_time
                .duration_since(spans[0].start_time)
                .unwrap()
                >= Duration::from_millis(125)
        );
        assert!(
            spans[0]
                .attributes
                .contains(&KeyValue::new("zeal.operation_id", id))
        );
        assert!(
            spans[0]
                .attributes
                .contains(&KeyValue::new("zeal.outcome", "discarded"))
        );
    }

    #[test]
    fn a_stalled_receiver_does_not_block_recording_or_fill_an_unbounded_queue() {
        use std::io::Read;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (accepted, request) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buffer = [0; 1];
            socket.read_exact(&mut buffer).unwrap();
            accepted.send(()).unwrap();
            let _ = released.recv_timeout(Duration::from_secs(3));
        });
        let telemetry = Telemetry::http(&endpoint).unwrap();
        drop(telemetry.start("first", None));
        let provider = telemetry.provider.as_ref().unwrap().clone();
        let flush = thread::spawn(move || {
            let _ = provider.force_flush();
        });
        request.recv_timeout(Duration::from_secs(3)).unwrap();
        let (recorded, recording) = mpsc::channel();
        let producer = thread::spawn(move || {
            for _ in 0..1024 {
                drop(telemetry.start("run", None));
            }
            recorded.send(telemetry).unwrap();
        });
        let telemetry = recording.recv_timeout(Duration::from_secs(2)).unwrap();
        release.send(()).unwrap();
        producer.join().unwrap();
        server.join().unwrap();
        flush.join().unwrap();
        telemetry.shutdown();
    }
}

#[cfg(test)]
mod jaeger_smoke {
    use super::*;

    /// Opt-in check against an externally started OTLP receiver.
    #[test]
    #[ignore = "requires OTEL_EXPORTER_OTLP_ENDPOINT and a running Jaeger"]
    fn export_run_detail_to_jaeger() {
        let telemetry = Telemetry::from_env();
        assert!(telemetry.enabled(), "set a debug OTLP base URL first");
        let root = std::env::temp_dir().join(format!("zeal-otel-smoke-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("private-workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let data = root.join("data");
        let (record, dir) =
            crate::store::create(&data, &workspace, "build", "//private:target").unwrap();
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/run/ok");
        std::fs::copy(fixture.join("bep.json"), dir.join("bep.json")).unwrap();
        std::fs::copy(fixture.join("profile.gz"), dir.join("profile.gz")).unwrap();
        crate::store::finish(&data, &workspace, &record.id, 0).unwrap();
        let mut operation = telemetry.start("run.detail", None);
        operation.run_id(&record.id);
        let id = operation.id.clone().unwrap();
        let detail = crate::ingest::run_detail(&data, &workspace, &record.id, &operation);
        operation.result(detail).unwrap();
        drop(operation);
        telemetry.record_ui(UiOperation {
            command: UiCommand::GetRun,
            operation_id: id.clone(),
            duration_ms: 25.0,
            outcome: UiOutcome::Applied,
        });
        telemetry.shutdown();
        std::fs::remove_dir_all(root).unwrap();
        eprintln!("Jaeger operation ID: {id}");
    }
}
