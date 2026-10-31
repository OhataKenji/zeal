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
        assert!(
            crate::console::read(private, &crate::state::Core::new(), "private-token", 0).is_err()
        );
        let stream = crate::protocol::bes::StreamId {
            build_id: "private-token".into(),
            invocation_id: "private-token".into(),
            component: 3,
        };
        assert!(crate::store::events::Receipt::begin(private, &stream).is_err());
    });
    provider.force_flush().unwrap();
    provider.shutdown().unwrap();
}
