# Development

Tauri v2 (Rust + system WebView) app. Frontend is Vite + Svelte 5 + TypeScript.

See the [frontend README](../src/README.md) for state ownership and the UI update flow.

## Getting started

Tool versions are pinned with [mise](https://mise.jdx.dev/).

```sh
mise install   # installs the pinned tools (trusts mise.toml on first run)
pnpm install
pnpm tauri dev    # run the app in dev mode
pnpm tauri build -- --locked  # build a release bundle
```

Commits are scanned for secrets by [gitleaks](https://github.com/gitleaks/gitleaks)
via [lefthook](https://lefthook.dev) (installed on `pnpm install`).

## Checks

Checks are defined as [mise tasks](https://mise.jdx.dev/tasks/) in `mise.toml`.

```sh
mise run check           # all checks (frontend, then rust)
mise run check:frontend  # type check, tests, format check, lint, build
mise run check:rust      # cargo fmt --check, clippy, test (src-tauri/)
mise run fmt             # auto-format (oxfmt + cargo fmt)
```

All of the above run in CI (`.github/workflows/`).

The modules graph parser is tested against a fixture captured from a real
`bazel mod graph --output=json` run (`src-tauri/tests/fixtures/`). `bazel mod
explain` needs an explicit `--depth`: its default view hides transitive
modules entirely (verified on Bazel 7, 8 and 9).

## Development traces

Start [Jaeger](https://www.jaegertracing.io/docs/2.22/getting-started/) with OTLP/HTTP on port 4318, then run:

```sh
ZELITH_OTLP_ENDPOINT=http://127.0.0.1:4318 pnpm tauri dev
```

View native timings under `zelith-dev` in [Jaeger](http://localhost:16686).
Export is debug-only and opt-in. Inputs, artifacts and inherited OTLP credentials stay local.
Background threads use separate traces; exit may drop traces.

Each BES stream has a `bes.stream` trace. `bes.receive` includes time waiting
for Bazel to send events and close its request stream. Its `append_ms` field
totals the time spent waiting for event writes. `bes.save` covers the
blocking-worker queue, final persistence after EOF, and the history notification.
`bes.ack` records when Zelith queues acknowledgements for the gRPC response,
including response backpressure. Bazel's logs report whether it received them.
Measure the receiver's tail from `bes.receive.build_finished` to the end
of `bes.ack`. Profile copying (`bes.save.profile`) happens before EOF.
