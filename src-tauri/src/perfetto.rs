//! Ephemeral loopback origin serving the bundled Perfetto UI.
//!
//! The viewer is a fixed set of digest-pinned assets fetched at build time;
//! this server exposes exactly those names on `127.0.0.1` with an ephemeral
//! port, a strict `Host` check against DNS rebinding, the viewer CSP, and no
//! CORS or write endpoints. Invocation traces never pass through it — the
//! frontend transfers them over `postMessage` after a PING/PONG handshake.

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use std::{
    collections::HashSet,
    net::Ipv4Addr,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, OnceLock},
};
use tauri::{AppHandle, Manager};
use tokio::{net::TcpListener, sync::OnceCell};

/// Response policy for every viewer document and asset. `frame-ancestors`
/// names the only pages allowed to embed the viewer: the packaged app origin
/// and the Vite dev server used by `tauri dev`.
const VIEWER_CSP: &str = "default-src 'self'; connect-src 'self' blob:; worker-src 'self' blob:; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; object-src 'none'; frame-ancestors tauri://localhost http://tauri.localhost https://tauri.localhost http://localhost:1420 http://127.0.0.1:1420; base-uri 'none'; form-action 'none'";

/// Lazily bound loopback listener, managed as Tauri state. The origin is
/// stable for the process lifetime once resolved.
pub struct PerfettoServer {
    origin: OnceCell<String>,
}

impl PerfettoServer {
    pub fn new() -> Self {
        Self {
            origin: OnceCell::new(),
        }
    }
}

struct Serve {
    root: PathBuf,
    host: String,
}

/// Resource names the pinned manifest allows, including `index.html`.
fn manifest_assets() -> &'static HashSet<String> {
    static ASSETS: OnceLock<HashSet<String>> = OnceLock::new();
    ASSETS.get_or_init(|| {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../resources/perfetto-manifest.json"))
                .expect("perfetto manifest must be valid JSON");
        manifest["resources"]
            .as_object()
            .expect("perfetto manifest must list resources")
            .keys()
            .cloned()
            .collect()
    })
}

fn mime(name: &str) -> &'static str {
    match FsPath::new(name).extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("wasm") => "application/wasm",
        Some("woff2") => "font/woff2",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("md") => "text/markdown; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn respond(status: StatusCode, body: Body, content_type: Option<&'static str>) -> Response {
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(VIEWER_CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(kind) = content_type {
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    }
    response
}

async fn index(State(serve): State<Arc<Serve>>, headers: HeaderMap) -> Response {
    serve_asset(&serve, &headers, "index.html").await
}

async fn asset(
    State(serve): State<Arc<Serve>>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Response {
    let name = if path.is_empty() {
        "index.html"
    } else {
        path.as_str()
    };
    serve_asset(&serve, &headers, name).await
}

async fn serve_asset(serve: &Serve, headers: &HeaderMap, name: &str) -> Response {
    // Only requests addressed to this listener's exact loopback authority
    // are served; anything else is a rebinding or stray probe.
    if headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        != Some(serve.host.as_str())
    {
        return respond(StatusCode::FORBIDDEN, Body::empty(), None);
    }
    // `embed.html` is the tracked first-party bootstrap; every other name
    // must come from the pinned upstream manifest.
    if name != "embed.html" && !manifest_assets().contains(name) {
        return respond(StatusCode::NOT_FOUND, Body::empty(), None);
    }
    match tokio::fs::read(serve.root.join(name)).await {
        Ok(bytes) => respond(StatusCode::OK, Body::from(bytes), Some(mime(name))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            respond(StatusCode::NOT_FOUND, Body::empty(), None)
        }
        Err(_) => respond(StatusCode::INTERNAL_SERVER_ERROR, Body::empty(), None),
    }
}

fn router(root: PathBuf, port: u16) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/{*path}", get(asset))
        .with_state(Arc::new(Serve {
            root,
            host: format!("127.0.0.1:{port}"),
        }))
}

/// Bundled resources win; the manifest-dir copy serves `cargo run`/`tauri
/// dev` and tests, where nothing is bundled yet. The source-tree fallback is
/// compiled out of release builds.
fn asset_root(app: &AppHandle) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join("perfetto"));
    }
    #[cfg(debug_assertions)]
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("perfetto"),
    );
    for root in candidates {
        if root.join("index.html").is_file() {
            return Ok(root);
        }
    }
    Err("Perfetto viewer assets are not installed; run `pnpm dev` or `pnpm build` first".into())
}

async fn serve(listener: TcpListener, app: Router) {
    if let Err(error) = axum::serve(listener, app).await {
        log::warn!("Perfetto viewer server stopped: {error}");
    }
}

/// Resolve (binding on first use) and return the loopback origin.
pub async fn origin(app: &AppHandle, server: &PerfettoServer) -> Result<String, String> {
    server
        .origin
        .get_or_try_init(|| async {
            let root = asset_root(app)?;
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .map_err(|error| format!("cannot bind the Perfetto viewer: {error}"))?;
            let port = listener
                .local_addr()
                .map_err(|error| format!("cannot read the Perfetto viewer address: {error}"))?
                .port();
            let app = router(root, port);
            tokio::spawn(serve(listener, app));
            Ok(format!("http://127.0.0.1:{port}"))
        })
        .await
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Minimal fixture: the permitted names plus `private.txt`, a real file
    /// on disk that the manifest whitelist must still refuse.
    fn fixture() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("zelith-perfetto-{}", uuid::Uuid::new_v4()));
        let assets = root.join("perfetto");
        for name in ["index.html", "embed.html", "trace_processor.wasm"] {
            let path = assets.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("fixture:{name}")).unwrap();
        }
        std::fs::write(assets.join("private.txt"), "fixture:private.txt").unwrap();
        (root, assets)
    }

    async fn serve_fixture(root: PathBuf) -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve(listener, router(root, port)));
        port
    }

    /// Raw HTTP/1.0-style exchange; `Connection: close` ends the response.
    async fn exchange(port: u16, request: &str) -> String {
        let mut socket = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        socket.read_to_end(&mut raw).await.unwrap();
        String::from_utf8_lossy(&raw).into_owned()
    }

    fn request(port: u16, method: &str, path: &str) -> String {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        )
    }

    async fn expect_status(port: u16, request: String, status: &str) {
        let response = exchange(port, &request).await;
        assert!(response.starts_with(status), "{request}: {response}");
    }

    #[tokio::test]
    async fn serves_manifest_assets_with_viewer_headers() {
        let (root, assets) = fixture();
        let port = serve_fixture(assets).await;
        let response = exchange(port, &request(port, "GET", "/")).await;
        let lower = response.to_lowercase();
        assert!(lower.starts_with("http/1.1 200"), "{response}");
        assert!(lower.contains("content-type: text/html"), "{response}");
        assert!(lower.contains("content-security-policy:"), "{response}");
        assert!(
            lower.contains("x-content-type-options: nosniff"),
            "{response}"
        );
        assert!(response.ends_with("fixture:index.html"), "{response}");

        let response = exchange(port, &request(port, "GET", "/embed.html")).await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("fixture:embed.html"), "{response}");

        let response = exchange(port, &request(port, "GET", "/trace_processor.wasm")).await;
        assert!(
            response
                .to_lowercase()
                .contains("content-type: application/wasm"),
            "{response}"
        );
        assert!(
            response.ends_with("fixture:trace_processor.wasm"),
            "{response}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn denies_unlisted_traversal_and_foreign_hosts() {
        let (root, assets) = fixture();
        let port = serve_fixture(assets).await;
        // Unlisted names (even real files), traversal and foreign Hosts are
        // all refused; the wildcard route only accepts GET/HEAD, so writes
        // never reach a path.
        expect_status(port, request(port, "GET", "/private.txt"), "HTTP/1.1 404").await;
        expect_status(
            port,
            request(port, "GET", "/..%2f..%2fCargo.toml"),
            "HTTP/1.1 404",
        )
        .await;
        // A rebinding-style Host is refused before any path lookup.
        expect_status(
            port,
            "GET /index.html HTTP/1.1\r\nHost: attacker.example\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
                .to_string(),
            "HTTP/1.1 403",
        )
        .await;
        expect_status(port, request(port, "POST", "/trace"), "HTTP/1.1 405").await;
        let _ = std::fs::remove_dir_all(&root);
    }
}
