//! Development-only tracing subscriber and OTLP export lifecycle.
//! https://docs.rs/tracing-opentelemetry/latest/tracing_opentelemetry/#examples

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider;
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response};
use opentelemetry_otlp::{Protocol, RetryPolicy, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::time::Duration;
use tauri::Manager;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;

const EXPORT_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Debug)]
struct TraceHttpClient(reqwest::blocking::Client);

#[async_trait::async_trait]
impl HttpClient for TraceHttpClient {
    async fn send_bytes(&self, mut request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        // The exporter imports ambient OTLP credentials even with an explicit endpoint.
        request.headers_mut().clear();
        request
            .headers_mut()
            .insert("content-type", "application/x-protobuf".parse()?);
        self.0.send_bytes(request).await
    }
}

pub fn init(app: &tauri::App) {
    if let Some(provider) = from_env() {
        app.manage(provider);
    }
}

pub fn shutdown(app: &tauri::AppHandle) {
    if let Some(provider) = app.try_state::<SdkTracerProvider>() {
        let _ = provider.shutdown_with_timeout(Duration::from_secs(1));
    }
}

fn from_env() -> Option<SdkTracerProvider> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let endpoint = std::env::var("ZEAL_OTLP_ENDPOINT").ok()?;
    let provider = provider(&endpoint).or_else(|| {
        // Exporter errors may contain the endpoint or credentials.
        log::warn!("development tracing disabled: invalid OTLP configuration");
        None
    })?;
    tracing::subscriber::set_global_default(subscriber(&provider)).ok()?;
    Some(provider)
}

// Export only Zeal's explicit operation instrumentation, never dependency events.
fn subscriber(provider: &SdkTracerProvider) -> impl tracing::Subscriber + Send + Sync + 'static {
    let layer = tracing_opentelemetry::layer()
        .with_tracer(provider.tracer("zeal"))
        .with_location(false)
        .with_threads(false)
        .with_target(false)
        .with_level(false)
        .with_tracked_inactivity(false)
        .with_context_activation(false)
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.is_span() && metadata.target() == "zeal::operations"
        }));
    tracing_subscriber::registry().with(layer)
}

fn provider(base: &str) -> Option<SdkTracerProvider> {
    let endpoint = trace_endpoint(base)?;
    // Blocking reqwest clients must be constructed outside a Tokio context.
    let client = std::thread::Builder::new()
        .spawn(|| {
            reqwest::blocking::Client::builder()
                .timeout(EXPORT_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
        })
        .ok()?
        .join()
        .ok()?
        .ok()?;
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_http_client(TraceHttpClient(client))
        .with_endpoint(endpoint)
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(EXPORT_TIMEOUT)
        .with_retry_policy(RetryPolicy::default().with_max_retries(0))
        .build()
        .ok()?;
    Some(
        SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
            // Do not read resource attributes from the developer's environment.
            .with_resource(
                Resource::builder_empty()
                    .with_service_name("zeal-dev")
                    .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
                    .build(),
            )
            .build(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn only_the_dedicated_endpoint_enables_development_tracing() {
        fn probe(endpoint: Option<&str>, expected: bool) {
            let mut process = isolated_probe("telemetry::tests::endpoint_env_probe");
            process
                .env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:9")
                .env("ZEAL_TEST_EXPECT_ENABLED", expected.to_string());
            if let Some(endpoint) = endpoint {
                process.env("ZEAL_OTLP_ENDPOINT", endpoint);
            }
            let output = process.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        probe(None, false);
        probe(Some("http://127.0.0.1:4318"), cfg!(debug_assertions));
        probe(Some(""), false);
    }

    #[test]
    #[ignore = "invoked with an isolated environment by the endpoint test"]
    fn endpoint_env_probe() {
        let provider = from_env();
        assert_eq!(
            provider.is_some(),
            std::env::var("ZEAL_TEST_EXPECT_ENABLED").unwrap() == "true"
        );
        if let Some(provider) = provider {
            provider.shutdown().unwrap();
        }
    }

    // SDK environment reads and the global subscriber are isolated in a fresh process.
    fn isolated_probe(name: &str) -> std::process::Command {
        let mut process = std::process::Command::new(std::env::current_exe().unwrap());
        process.args(["--exact", name, "--ignored"]).env_clear();
        process
    }

    #[cfg(debug_assertions)]
    async fn exported_request(header_variable: &'static str) -> String {
        use axum::Router;
        use axum::http::HeaderMap;
        use axum::routing::post;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (send, mut receive) = tokio::sync::mpsc::channel(1);
        let app = Router::new().route(
            "/v1/traces",
            post(move |headers: HeaderMap, body: Bytes| async move {
                send.send(format!("{headers:?}{}", String::from_utf8_lossy(&body)))
                    .await
                    .unwrap();
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let output = tokio::task::spawn_blocking(move || {
            isolated_probe("telemetry::tests::http_export_probe")
                .env("ZEAL_OTLP_ENDPOINT", endpoint)
                .env("OTEL_BSP_SCHEDULE_DELAY", "60000") // Export only on force_flush.
                .env(
                    header_variable,
                    "x-review-secret=private-token,content-type=private-token",
                )
                .env("OTEL_RESOURCE_ATTRIBUTES", "review.private=private-token")
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(output.status.success(), "{output:?}");
        let request = tokio::time::timeout(Duration::from_secs(3), receive.recv()).await;
        server.abort();
        let request = request.unwrap().unwrap();
        assert!(request.contains("console.read"));
        request
    }

    #[tokio::test]
    #[cfg(debug_assertions)]
    async fn generic_otlp_credentials_are_not_exported() {
        let request = exported_request("OTEL_EXPORTER_OTLP_HEADERS").await;
        assert!(!request.contains("private-token"));
        assert!(!request.contains("history record or artifact is missing"));
        assert!(!request.contains("StoreError"));
        assert!(!request.contains(env!("CARGO_MANIFEST_DIR")));
        assert!(request.contains("application/x-protobuf"));
    }

    #[tokio::test]
    #[cfg(debug_assertions)]
    async fn trace_otlp_credentials_are_not_exported() {
        let request = exported_request("OTEL_EXPORTER_OTLP_TRACES_HEADERS").await;
        assert!(!request.contains("private-token"));
        assert!(!request.contains("history record or artifact is missing"));
        assert!(!request.contains("StoreError"));
        assert!(!request.contains(env!("CARGO_MANIFEST_DIR")));
        assert!(request.contains("application/x-protobuf"));
    }

    #[test]
    #[ignore = "invoked with isolated environment by the HTTP credential tests"]
    #[cfg(debug_assertions)]
    fn http_export_probe() {
        let provider = from_env().expect("set a debug ZEAL_OTLP_ENDPOINT");
        tracing::info_span!(target: "zeal::operations", "http.export_probe").in_scope(|| {
            tracing::error!(target: "dependency", "private-token");
            tracing::error!(target: "zeal::operations", "private-token");
            // Arguments and returned errors must not become span fields.
            let private = std::path::Path::new("private-token");
            assert!(crate::store::read_record(private, private, "private-token").is_err());
            assert!(crate::console::read(private, private, "private-token", 0, true).is_err());
            assert!(crate::store::events::get(private, private, "private-token").is_err());
            let event = crate::protocol::bes::OrderedBuildEvent {
                event: Some(crate::protocol::bes::BuildEvent {
                    bazel_event: Some(prost_types::Any {
                        type_url: "private-token".into(),
                        value: b"private-token".to_vec(),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert!(crate::store::events::save(private, &event).is_err());
        });
        provider.force_flush().unwrap();
        provider.shutdown().unwrap();
    }
}
