# Zeal

A local first Bazel GUI client.

## Getting Started

Tauri v2 (Rust + system WebView) app. Frontend is Vite + Svelte 5 + TypeScript.

Tool versions are pinned with [mise](https://mise.jdx.dev/).

```sh
mise install   # installs the pinned tools (trusts mise.toml on first run)
pnpm install
pnpm tauri dev    # run the app in dev mode
pnpm tauri build  # build a release bundle
```

Commits are scanned for secrets by [gitleaks](https://github.com/gitleaks/gitleaks)
via [lefthook](https://lefthook.dev) (installed on `pnpm install`).

## UI preview

Run `pnpm dev`, then open `http://localhost:1420/?preview` for sample data
or `?preview=empty` for workspace setup.

## Checks

Checks are defined as [mise tasks](https://mise.jdx.dev/tasks/) in `mise.toml`.

```sh
mise run check           # all checks (frontend, then rust)
mise run check:frontend  # type check, tests, format check, lint, build
mise run check:rust      # cargo fmt --check, clippy, test (src-tauri/)
mise run fmt             # auto-format (oxfmt + cargo fmt)
```

All of the above run in CI (`.github/workflows/`).

See [docs/prd.md](docs/prd.md) and [docs/design/](docs/design/) for more details.

## Development traces

To inspect Zeal operations, start an external Jaeger with an OTLP/HTTP receiver:

```sh
docker run --rm --name zeal-jaeger \
  -p 127.0.0.1:4318:4318 -p 127.0.0.1:16686:16686 \
  cr.jaegertracing.io/jaegertracing/jaeger:2.22.0
```

Then start Zeal with an explicit OTLP **base URL**:

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 pnpm tauri dev
```

Open [Jaeger](http://localhost:16686) and select `zeal-dev`.
Workspace open/restore/refresh, build/test, stop, run lists and run details are
traced. Expand a run to separate process wait from result saving, or a detail
request to inspect log/BEP/profile reads. Query XML parsing overlaps the Bazel
process, so do not add those durations together.

The URL is read only in debug builds. Without it, tracing is disabled; release
builds ignore it. Zeal sends OTLP/HTTP protobuf to `<base>/v1/traces`.
The protocol, service name, queue size and timeouts are fixed for development.
Signal-specific endpoint/protocol overrides are not used. For a remote receiver,
use HTTPS; OTLP header environment variables can supply authentication.

UI traces carry an allowlisted operation, outcome and monotonic duration through
Svelte's next update. They share `zeal.operation_id` with native traces; search
that tag to correlate them. They are separate traces, and UI timestamps are
anchored at receipt, not exact paint timings. Query completion adds a
`ui.state_applied` record with the originating operation ID. Native run traces
continue through background completion, even after the start command returns.
Use `zeal.session_id` for one app launch and `zeal.run_id` for one saved run.

Only fixed classifications, random IDs, counts, durations, app version and OS
are exported. Workspace paths, targets, command arguments, search text, error
messages, environment resource attributes and raw log/BEP/profile data stay
local. Existing logs are not forwarded. This exports **traces and span events**,
not OTel Logs or Metrics. Jaeger runs independently of Zeal.

Export is best-effort: a 256-span SDK queue, batches of 32, a 750 ms request
timeout and no retries. UI reports have at most 32 pending requests. Overflow
and failed exports are dropped. Exit stops the child first, then gives tracing
at most one additional second to shut down. Forced exit may lose traces.
Jaeger's example uses memory storage and loses data on restart.

See [Jaeger setup](https://www.jaegertracing.io/docs/2.22/getting-started/).

## License

[Apache-2.0](LICENSE)
