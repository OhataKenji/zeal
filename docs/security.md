# Frontend security

Zeal loads its frontend from bundled files. Its Content Security Policy (CSP)
blocks remote scripts, images, connections, frames, and HTML form submissions.
The exceptions are Tauri IPC, local application assets, SVG image blobs, and
inline styles required by the terminal renderer. JavaScript string evaluation
and unapproved inline scripts remain blocked. Tauri's automatic script nonces and hashes
remain enabled.

Do not enable `freezePrototype` without retesting xterm: freezing
`Object.prototype` currently prevents xterm 6 from loading.

The Graphviz worker has a separate CSP because file-backed workers do not
inherit the document's policy. Its bundled Wasm may compile, but the worker may
not connect to a server, import another script, or create another worker. Keep
Graphviz output in an `<img>`; do not insert its SVG into the document as HTML.

Development uses the Vite server at `http://localhost:1420`. The document may
connect to its HMR WebSocket. Vite supplies the worker policy separately and
permits same-origin module imports, which are absent from the bundled worker.
The worker itself does not need HMR connections.

## Boundaries

CSP reduces script-injection opportunities. It does not distinguish malicious
JavaScript from legitimate code once that JavaScript is running in the same
window. Rust commands must validate their inputs. Navigation and new-window
requests must not provide a way to open remote content from the WebView.

CSP does not sandbox Rust, Bazel, repository code, or child processes. Open only
trusted repositories. Do not treat a blocked browser request as proof that the
application cannot send data through a native command.

## Verification

Run the normal frontend and Rust checks, then check a bundled application on
each supported operating system:

- Load the workspace, fetch targets, and start and stop an invocation through
  IPC. Check terminal input, colors, resizing, saved logs, and timing views.
- Render a Graphviz result and reopen the enlarged image.
- Confirm that inline script execution, JavaScript string evaluation, remote
  fetches, form submissions, external navigation, and new windows are blocked.
- Check the worker response's CSP. Confirm that Graphviz still renders while
  worker fetches, JavaScript string evaluation, and child workers are blocked.
- In development, verify HMR and Graphviz after a source edit. Keep development
  network allowances out of the production policy.

A browser preview can test CSP behavior, but it does not verify Tauri's custom
protocol or the operating system's WebView. Recheck those boundaries after
Tauri, Vite, Graphviz, or terminal renderer upgrades.
