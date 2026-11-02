//! WebView boundaries beyond the document's CSP.

use std::borrow::Cow;
use tauri::http::{Request, Response, header};
use tauri::{App, Url, WebviewWindowBuilder};

// Viz embeds its Wasm bytes. Workers need neither network access nor more scripts.
const WORKER_CSP: &str = "default-src 'none'; script-src 'wasm-unsafe-eval'";

pub fn create_window(app: &App) -> tauri::Result<()> {
    let config = app
        .config()
        .app
        .windows
        .first()
        .expect("main window configuration");
    // Match Tauri's custom-protocol origin on each platform.
    let origin = if cfg!(windows) || cfg!(target_os = "android") {
        if config.use_https_scheme {
            "https://tauri.localhost/"
        } else {
            "http://tauri.localhost/"
        }
    } else {
        "tauri://localhost/"
    };
    let app_url = if tauri::is_dev() {
        app.config().build.dev_url.clone()
    } else {
        None
    }
    .unwrap_or_else(|| Url::parse(origin).expect("valid app origin"));

    WebviewWindowBuilder::from_config(app, config)?
        .on_navigation(move |url| navigation_allowed(url, &app_url))
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_web_resource_request(|request, response| restrict_scripts(&request, response))
        .build()?;
    Ok(())
}

fn navigation_allowed(url: &Url, app_url: &Url) -> bool {
    url.scheme() == app_url.scheme()
        && url.host_str() == app_url.host_str()
        && url.port_or_known_default() == app_url.port_or_known_default()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/" | "/index.html")
        && url.query().is_none()
        && url.fragment().is_none()
}

fn restrict_scripts(request: &Request<Vec<u8>>, response: &mut Response<Cow<'static, [u8]>>) {
    // Tauri normalizes its platform-specific URLs before invoking this hook.
    // External URLs (including the dev server) must not be mistaken for assets.
    if request.uri().scheme_str() != Some("tauri")
        || request
            .uri()
            .authority()
            .map(|authority| authority.as_str())
            != Some("localhost")
    {
        return;
    }
    let mime = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if matches!(mime, Some("text/javascript" | "application/javascript")) {
        // Script responses define the policy of workers, not scripts in the page.
        // Cover all JS assets so a renamed or added worker cannot lose its policy.
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static(WORKER_CSP),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{WORKER_CSP, navigation_allowed, restrict_scripts};
    use std::borrow::Cow;
    use tauri::Url;
    use tauri::http::{Request, Response, header};

    fn allowed(candidate: &str, entry: &str) -> bool {
        navigation_allowed(&Url::parse(candidate).unwrap(), &Url::parse(entry).unwrap())
    }

    #[test]
    fn permits_only_the_app_entry_documents() {
        assert!(allowed("tauri://localhost/", "tauri://localhost/"));
        assert!(allowed(
            "tauri://localhost/index.html",
            "tauri://localhost/"
        ));
        assert!(!allowed(
            "tauri://localhost/assets/app.js",
            "tauri://localhost/"
        ));
        assert!(!allowed(
            "tauri://localhost/?secret=value",
            "tauri://localhost/"
        ));
        assert!(!allowed(
            "tauri://localhost/#fragment",
            "tauri://localhost/"
        ));
    }

    #[test]
    fn rejects_other_origins_and_credentials() {
        assert!(!allowed("https://example.org/", "tauri://localhost/"));
        assert!(!allowed(
            "tauri://localhost.example.org/",
            "tauri://localhost/"
        ));
        assert!(!allowed("tauri://localhost:1420/", "tauri://localhost/"));
        assert!(!allowed("tauri://user@localhost/", "tauri://localhost/"));
        assert!(!allowed(
            "tauri://user:password@localhost/",
            "tauri://localhost/"
        ));
        assert!(!allowed("javascript:alert(1)", "tauri://localhost/"));
        assert!(!allowed("data:text/html,hello", "tauri://localhost/"));
    }

    #[test]
    fn dev_navigation_is_limited_to_the_configured_server() {
        assert!(allowed("http://localhost:1420/", "http://localhost:1420/"));
        assert!(!allowed("http://localhost:1421/", "http://localhost:1420/"));
        assert!(!allowed("http://127.0.0.1:1420/", "http://localhost:1420/"));
        assert!(!allowed(
            "https://localhost:1420/",
            "http://localhost:1420/"
        ));
        assert!(!allowed(
            "http://localhost:1420/@fs/private",
            "http://localhost:1420/"
        ));
    }

    fn response(mime: &str) -> Response<Cow<'static, [u8]>> {
        Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .body(Cow::Borrowed(&[] as &[u8]))
            .unwrap()
    }

    fn request(uri: &str) -> Request<Vec<u8>> {
        Request::builder().uri(uri).body(Vec::new()).unwrap()
    }

    #[test]
    fn protects_javascript_regardless_of_the_chunk_name() {
        let mut asset = response("text/javascript; charset=utf-8");
        restrict_scripts(
            &request("tauri://localhost/assets/renamed-worker.js"),
            &mut asset,
        );
        assert_eq!(asset.headers()[header::CONTENT_SECURITY_POLICY], WORKER_CSP);
    }

    #[test]
    fn preserves_the_documents_generated_csp() {
        let mut html = response("text/html");
        let policy = "script-src 'self' 'nonce-generated-by-tauri'";
        html.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static(policy),
        );
        restrict_scripts(&request("tauri://localhost/index.html"), &mut html);
        assert_eq!(html.headers()[header::CONTENT_SECURITY_POLICY], policy);
    }

    #[test]
    fn does_not_rewrite_external_resources() {
        let mut external = response("text/javascript");
        restrict_scripts(&request("https://example.org/worker.js"), &mut external);
        assert!(
            !external
                .headers()
                .contains_key(header::CONTENT_SECURITY_POLICY)
        );
    }
}
