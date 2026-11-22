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

## Browser preview

`pnpm dev` serves the app shell in a plain browser. Open
`http://localhost:1420/?preview` to mount the development preview, which
answers Tauri IPC from fixtures captured from the
[parcel-desk example](../examples/parcel-desk) (`src/preview/`):

- `history.json` — three real invocations (build `//...`, build
  `//rates:rate_server`, test `//dispatch:quote_contract_test`) with their saved log,
  BEP and `--profile` data under illustrative invocation IDs. The fixture
  stores `startedAt`/`finishedAt` as millisecond offsets from the first
  capture's start; the preview rebases them so the newest capture ends now.
  Profile lanes and phases keep per-run microsecond timing while
  system-wide counters are omitted, so the History → Timing view shows the
  real capture timeline. For public distribution the fixtures strip local
  paths and machine-specific flags (BES backend, profile and event-file
  paths) from logs and command lines.
- `workspace.json` — the example's `bazel query` targets and edges,
  `bazel mod graph` snapshot, workspace files, `aquery` actions (the fixture
  abbreviates long argument lists), `cquery` outputs and transitions, and a
  `bazel info` snapshot: every key is present, machine-specific paths are
  rebased under `/demo`, and host/JVM/resource fields are marked
  `(not retained in preview)`. `queries.json` holds the sidebar presets'
  captured query output.

Everything the preview serves is real captured data except runs started from
the UI: simulated build/test/run invocations carry no BEP or profile (both
reads report missing), finish with real elapsed time only, and their Console
is labeled as a simulation. Analytics is also partly synthetic: besides the
real invocations it reports deterministic activity for the 29 local calendar
days before the captures, generated from the captured Parcel Desk commands
and durations, and those rows stay isolated from History and Timing. The
browser never runs Bazel: the Query page
replays the sidebar presets from their captures and projects simple
target patterns (`//...`, `//pkg/...`, `//pkg:all`, exact labels) from the
captured graph — anything else is refused rather than guessed. Query and
target reads cover only the captured default configuration and reject build
flags rather than guessing at configurations that were never captured.
`?preview=empty` opens the preview with no workspace, for the empty-state
flow.

## Development traces

Start [Jaeger](https://www.jaegertracing.io/docs/2.22/getting-started/) with OTLP/HTTP on port 4318, then run:

```sh
ZEAL_OTLP_ENDPOINT=http://127.0.0.1:4318 pnpm tauri dev
```

View native timings under `zeal-dev` in [Jaeger](http://localhost:16686).
Export is debug-only and opt-in. Inputs, artifacts and inherited OTLP credentials stay local.
Background threads use separate traces; exit may drop traces.

Each BES stream has a `bes.stream` trace. `bes.receive` includes time waiting
for Bazel to send events and close its request stream. Its `append_ms` field
totals the time spent waiting for event writes. `bes.save` covers the
blocking-worker queue, final persistence after EOF, and the history notification.
`bes.ack` records when Zeal queues acknowledgements for the gRPC response,
including response backpressure. Bazel's logs report whether it received them.
Measure the receiver's tail from `bes.receive.build_finished` to the end
of `bes.ack`. Profile copying (`bes.save.profile`) happens before EOF.
