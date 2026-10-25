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

## Record terminal builds

Add to `.bazelrc` and use `bazel build --config=zeal //...` while Zeal is open:

```bazelrc
build:zeal --bes_backend=grpc://127.0.0.1:4718
build:zeal --bes_timeout=5s
```

## License

[Apache-2.0](LICENSE)
