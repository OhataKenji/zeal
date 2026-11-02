//! Apply CSP to workers as well as the document.

use std::borrow::Cow;
use tauri::http::{Request, Response, header};
use tauri::{App, WebviewWindowBuilder};

// Viz embeds its Wasm bytes. Workers need neither network access nor more scripts.
const WORKER_CSP: &str = "default-src 'none'; script-src 'wasm-unsafe-eval'";

pub fn create_window(app: &App) -> tauri::Result<()> {
    let config = app
        .config()
        .app
        .windows
        .first()
        .expect("main window configuration");
    // Build manually so the worker policy is attached before any asset loads.
    WebviewWindowBuilder::from_config(app, config)?
        .on_web_resource_request(|request, response| restrict_scripts(&request, response))
        .build()?;
    Ok(())
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
    use super::{WORKER_CSP, restrict_scripts};
    use std::borrow::Cow;
    use tauri::http::{Request, Response, header};

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
