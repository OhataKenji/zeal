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

## Live runs and terminal builds

Choose `build`, `test`, or `run` to open a temporary **Console** backed by a PTY.
Type directly in the terminal; Enter, arrow keys, Ctrl+C and Ctrl+D work as they
do in a terminal. Switching away from Console hides its view; the process keeps running.
Use Stop to end the process. Console has no sidebar entry.

After completion, inspect saved output, results and Timing in **Runs**.
Console immediately opens that run in Runs → All when it finishes.
The Logs section comes first, at the same position as the terminal.
If you navigate away during execution, completion keeps your current view.
Zeal currently runs one command at a time. Quitting the app stops its active run.

To record terminal builds, add this to your workspace's `.bazelrc`:

```bazelrc
build:zeal --bes_backend=grpc://127.0.0.1:4718
build:zeal --bes_timeout=5s
```

Keep Zeal open and run `bazel build --config=zeal //...` (`test` and `run` also
work). Open the same workspace in Zeal to see its records. Omit `--config=zeal`
when Zeal is closed. Zeal-launched commands configure the receiver automatically.

Build results and program exit results are separate: a successful build can
leave a `run` program waiting for input. Terminal-launched programs keep their
input, runtime output, and exit status in the terminal. BES records their build
events. Referenced external profiles and test-log files are not imported.
Runs shows the last 512 KiB of output; the full terminal byte stream and original
BES events remain in local storage. Console retains 5,000 scrollback lines.
External builds appear only in Runs and never create a Console.
The local receiver accepts up to four concurrent requests or streams. Stored BES
data is limited to 512 MiB per invocation and 4 GiB or one million events in total.
New events beyond these limits receive `RESOURCE_EXHAUSTED`; existing records
remain available. Result summaries above 8 MiB are marked incomplete; accepted
original events remain saved.
Analytics continues to count Zeal-launched build/test
runs. See [the architecture](docs/design/m6-live-runs-and-bes.md).

The optional real-Bazel integration test uses a temporary workspace:

```sh
ZEAL_TEST_BAZEL=/absolute/path/to/bazel cargo test --manifest-path src-tauri/Cargo.toml \
  real_bazel_ -- --ignored
```

## License

[Apache-2.0](LICENSE)
