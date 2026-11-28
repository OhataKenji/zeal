//! Custom `perfetto://` scheme serving the bundled Perfetto UI.
//!
//! The viewer is a fixed set of digest-pinned assets fetched at build time;
//! the scheme exposes exactly those names plus the first-party `embed.html`
//! bootstrap — never directories or arbitrary paths. Invocation traces are
//! not path-addressable: `prepare_invocation_timing` opens the validated
//! artifact and issues a random one-shot grant, and only
//! `/trace/{token}/profile.json.gz` consumes it.

use std::{
    collections::HashSet,
    fs::File,
    io,
    path::{Path as FsPath, PathBuf},
    sync::{Mutex, OnceLock},
};

use tauri::{
    AppHandle, Manager,
    http::{Method, Request, Response, StatusCode, header},
};
use uuid::Uuid;

/// Viewer origin per platform: custom schemes surface as
/// `<scheme>://localhost` on macOS, iOS and Linux, and as
/// `http://<scheme>.localhost` on Windows and Android.
#[cfg(any(target_os = "windows", target_os = "android"))]
pub(crate) const VIEWER_ORIGIN: &str = "http://perfetto.localhost";
#[cfg(not(any(target_os = "windows", target_os = "android")))]
pub(crate) const VIEWER_ORIGIN: &str = "perfetto://localhost";

/// Response policy for every viewer document and asset. `frame-ancestors`
/// names the only pages allowed to embed the viewer: the packaged app origin
/// and the Vite dev server used by `tauri dev`.
const VIEWER_CSP: &str = "default-src 'self'; connect-src 'self' blob:; worker-src 'self' blob:; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; object-src 'none'; frame-ancestors tauri://localhost http://tauri.localhost https://tauri.localhost http://localhost:1420 http://127.0.0.1:1420; base-uri 'none'; form-action 'none'";

/// The single outstanding trace grant. `begin` reserves the slot before the
/// artifact finishes opening so a newer selection always supersedes an
/// in-flight one; `finish` installs the file only while the reservation is
/// current, and `take` consumes a matching token without holding the lock
/// during the read.
#[derive(Default)]
pub struct PendingTrace(Mutex<Option<(Uuid, Option<File>)>>);

impl PendingTrace {
    /// Claim the grant slot for a new selection, replacing any previous one.
    pub fn begin(&self) -> Uuid {
        let token = Uuid::new_v4();
        *self.0.lock().expect("pending trace lock poisoned") = Some((token, None));
        token
    }

    /// Install the opened artifact while `token` still holds the slot.
    pub fn finish(&self, token: Uuid, file: File) -> bool {
        let mut grant = self.0.lock().expect("pending trace lock poisoned");
        match grant.as_mut() {
            Some((issued, slot)) if *issued == token => {
                *slot = Some(file);
                true
            }
            _ => false,
        }
    }

    /// Consume the grant `token` names; wrong, stale or unfinished tokens
    /// get `None`.
    pub fn take(&self, token: Uuid) -> Option<File> {
        let mut grant = self.0.lock().expect("pending trace lock poisoned");
        if matches!(grant.as_ref(), Some((issued, _)) if *issued == token) {
            grant.take().and_then(|(_, file)| file)
        } else {
            None
        }
    }
}

/// What a viewer path resolves to.
enum Target {
    /// A bundled asset by resource name: `index.html`, the first-party
    /// `embed.html`, or a manifest-listed file.
    Asset(String),
    /// A pending trace grant token.
    Trace(Uuid),
}

fn resolve(path: &str) -> Option<Target> {
    let name = path.strip_prefix('/').unwrap_or(path);
    if name.is_empty() {
        return Some(Target::Asset("index.html".to_string()));
    }
    if let Some(token) = name
        .strip_prefix("trace/")
        .and_then(|rest| rest.strip_suffix("/profile.json.gz"))
        .and_then(|token| Uuid::parse_str(token).ok())
    {
        return Some(Target::Trace(token));
    }
    // `embed.html` is the tracked first-party bootstrap; every other name
    // must come from the pinned upstream manifest.
    if name == "embed.html" || manifest_assets().contains(name) {
        return Some(Target::Asset(name.to_string()));
    }
    None
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

fn reply(
    status: StatusCode,
    body: Vec<u8>,
    content_type: Option<&'static str>,
) -> Response<Vec<u8>> {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_SECURITY_POLICY, VIEWER_CSP)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(kind) = content_type {
        response = response.header(header::CONTENT_TYPE, kind);
    }
    response.body(body).expect("valid viewer response")
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    reply(status, Vec::new(), None)
}

async fn asset(app: &AppHandle, name: &str) -> Response<Vec<u8>> {
    let Ok(root) = asset_root(app) else {
        return empty(StatusCode::NOT_FOUND);
    };
    match tokio::fs::read(root.join(name)).await {
        Ok(bytes) => reply(StatusCode::OK, bytes, Some(mime(name))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => empty(StatusCode::NOT_FOUND),
        Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn trace(app: &AppHandle, token: Uuid) -> Response<Vec<u8>> {
    let Some(pending) = app.try_state::<PendingTrace>() else {
        return empty(StatusCode::NOT_FOUND);
    };
    // The grant is consumed before the read: wrong, replayed and failed
    // requests all surface the same 404.
    let Some(file) = pending.take(token) else {
        return empty(StatusCode::NOT_FOUND);
    };
    match tauri::async_runtime::spawn_blocking(move || crate::profile::read(file)).await {
        Ok(Ok(bytes)) => reply(StatusCode::OK, bytes, Some("application/octet-stream")),
        _ => empty(StatusCode::NOT_FOUND),
    }
}

/// Serve one viewer request, restricted to the main webview, `GET`, and the
/// exact trusted host for this scheme on this platform.
pub async fn handle(
    app: &AppHandle,
    main_webview: bool,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let uri = request.uri();
    let trusted = main_webview
        && *request.method() == Method::GET
        && match (uri.scheme_str(), uri.host()) {
            (Some("perfetto"), Some("localhost")) => true,
            // Wry presents Windows/Android custom schemes as
            // `http://<scheme>.localhost` to the handler.
            (Some("http"), Some("perfetto.localhost")) => true,
            _ => false,
        };
    let target = if trusted { resolve(uri.path()) } else { None };
    match target {
        Some(Target::Asset(name)) => asset(app, &name).await,
        Some(Target::Trace(token)) => trace(app, token).await,
        None => empty(StatusCode::NOT_FOUND),
    }
}

/// Bundled resources win; the manifest-dir copy serves `cargo run`/`tauri
/// dev` and tests, where nothing is bundled yet. The source-tree fallback is
/// compiled out of release builds.
pub fn asset_root(app: &AppHandle) -> Result<PathBuf, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn opened_file() -> File {
        File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap()
    }

    #[test]
    fn grants_are_reserved_installed_once_and_token_checked() {
        let pending = PendingTrace::default();

        // A newer reservation supersedes an in-flight open.
        let first = pending.begin();
        let second = pending.begin();
        assert!(!pending.finish(first, opened_file()));

        // An arbitrary token neither consumes nor reveals the grant.
        assert!(pending.take(Uuid::new_v4()).is_none());
        assert!(pending.finish(second, opened_file()));
        assert!(pending.take(second).is_some());
        // Consumed grants cannot be replayed.
        assert!(pending.take(second).is_none());
    }

    #[test]
    fn resolve_allows_only_manifest_names_embed_and_trace_grants() {
        for path in ["/", "/embed.html", "/trace_processor.wasm"] {
            assert!(resolve(path).is_some(), "{path}");
        }
        let token = Uuid::new_v4();
        assert!(matches!(
            resolve(&format!("/trace/{token}/profile.json.gz")),
            Some(Target::Trace(granted)) if granted == token
        ));
        for path in ["/private.txt", "/trace/not-a-uuid/profile.json.gz"] {
            assert!(resolve(path).is_none(), "{path}");
        }
    }
}
