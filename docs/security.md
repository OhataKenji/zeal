# Content Security Policy

The document CSP starts with `default-src 'none'`. It allows bundled scripts,
styles, workers, local images, SVG image blobs, and Tauri IPC. Inline styles are
allowed because xterm creates dynamic `<style>` elements. JavaScript string
compilation, unapproved inline scripts, remote resources, frames, and HTML form
submissions remain blocked. Tauri's automatic nonces and hashes remain enabled.

The Graphviz worker needs its own CSP: file-backed workers do not inherit the
parent document's policy. The bundled worker may compile its embedded Wasm but
cannot fetch data, import other scripts, or create child workers. Tauri adds
this policy to JavaScript asset responses, independently of the document CSP.

Development permits document connections to Vite and `ws://localhost:1420`
for HMR. Vite separately allows worker module imports from the same origin and
Wasm compilation, but no connections or child workers. Production stays stricter.

CSP does not contain top-level navigation or native effects through Rust IPC.
It cannot distinguish legitimate code from malicious code in the same window.
It does not sandbox Bazel or repository code. Open only trusted repositories.

## Verification

After the normal checks, verify a bundled application in each supported OS's
WebView, especially after Tauri, Vite, Graphviz, or xterm upgrades:

- Check IPC, terminal input and colors, saved logs, and timing views.
- Render a graph and reopen its enlarged image.
- Confirm unapproved inline scripts, JavaScript string compilation, remote
  fetches, and HTML form submissions are blocked in the document.
- Confirm worker fetches, string compilation, and child workers are blocked
  while Graphviz still renders. Inspect the worker response's CSP.
- In development, check Graphviz and HMR after a source edit.

A browser preview cannot verify Tauri's custom protocol or the native WebView.
