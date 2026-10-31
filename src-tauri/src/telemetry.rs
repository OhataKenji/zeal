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
mod tests;
