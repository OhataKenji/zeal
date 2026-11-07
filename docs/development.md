# Development

Tauri v2 (Rust + system WebView) app. Frontend is Vite + Svelte 5 + TypeScript.

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

## Development traces

Start [Jaeger](https://www.jaegertracing.io/docs/2.22/getting-started/) with OTLP/HTTP on port 4318, then run:

```sh
ZEAL_OTLP_ENDPOINT=http://127.0.0.1:4318 pnpm tauri dev
```

View native timings under `zeal-dev` in [Jaeger](http://localhost:16686).
Export is debug-only and opt-in. Inputs, artifacts and inherited OTLP credentials stay local.
Background threads use separate traces; exit may drop traces.
