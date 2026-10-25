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

## UI conventions

Use [shadcn-svelte](https://www.shadcn-svelte.com/) with the Vega style and neutral
palette. Keep the standard component styles; put screen layout in `src/app.css`
and theme tokens in `src/theme.css`. Use semantic tokens instead of per-screen
color palettes. Status colors describe outcomes, not decoration.

Button, Input, and Badge sources live in `src/lib/components/ui/`, with their
upstream MIT license. Add components only when a screen needs them. Keep existing
Phosphor icons and system fonts. The CLI and unused animation styles are not
project dependencies.

To add a component, run `pnpm dlx shadcn-svelte@1.7.1 add <component>`, then review
the generated source and dependency diff. Pin any new direct dependencies to exact
versions and commit the lockfile. Component source updates are explicit reviewed
changes; package updates do not replace these files.

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
