# Zeal

A local first Bazel GUI client.

## Getting Started

Tauri v2 (Rust + system WebView) app. Frontend is Vite + Svelte 5 + TypeScript.

```sh
pnpm install
pnpm tauri dev    # run the app in dev mode
pnpm tauri build  # build a release bundle
```

## Checks

Frontend:

```sh
pnpm check   # type-check (svelte-check + tsc)
pnpm lint    # oxlint
pnpm format  # oxfmt
```

Rust (`src-tauri/`):

```sh
cargo fmt --check
cargo clippy
cargo test
```

All of the above run in CI (`.github/workflows/`).

See [docs/prd.md](docs/prd.md) and [docs/design/](docs/design/) for more details.

## License

[Apache-2.0](LICENSE)
