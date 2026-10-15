# Zeal

A local first Bazel GUI client.

## Getting Started

Tauri v2 (Rust + system WebView) app. Frontend is Vite + Svelte 5 + TypeScript.

Tool versions (Node.js, pnpm, gitleaks, Rust) are pinned with
[mise](https://mise.jdx.dev/). `rust-toolchain.toml` is the source of truth for
Rust and is also honored by plain rustup. [Install
mise](https://mise.jdx.dev/install.html), then:

```sh
mise install   # installs the pinned tools (trusts mise.toml on first run)
pnpm install
pnpm tauri dev    # run the app in dev mode
pnpm tauri build  # build a release bundle
```

If mise is not activated in your shell, prefix commands with `mise exec --`.

On Linux, Tauri also needs system packages:

```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
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
