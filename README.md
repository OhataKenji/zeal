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

`pnpm dev` opens the frontend server. Visit `http://localhost:1420/?preview`
for an interactive browser preview with synthetic workspace, run and profile data.
`?preview=empty` shows workspace setup. Preview actions never invoke Bazel or
read local workspace data. The preview module is excluded from production builds and cannot replace the
IPC connection inside Tauri.

The UI uses the existing Svelte + Vite stack. It does not require SvelteKit.

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

## License

[Apache-2.0](LICENSE)
