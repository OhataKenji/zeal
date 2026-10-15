# Zeal

A local first Bazel GUI Client.

## Getting Started

Tauri v2 (Rust + system WebView) app. Frontend is Vite + Svelte 5 + TypeScript.

```sh
pnpm install
pnpm tauri dev    # run the app in dev mode
pnpm tauri build  # build a release bundle
pnpm check        # type-check the frontend
pnpm lint         # oxlint
pnpm format       # auto-format with oxfmt (verify: pnpm exec oxfmt --check)
```

Rust side (`src-tauri/`): `cargo fmt --check`, `cargo clippy`, `cargo test`. All of the above run in CI (`.github/workflows/ci.yml`).

See [docs/prd.md](docs/prd.md) and [docs/design/](docs/design/) for more details.

## License

[Apache-2.0](LICENSE)
