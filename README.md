<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Zeal logo" width="128">
</p>

<h1 align="center">Zeal</h1>

<p align="center">A local-first Bazel GUI client.</p>

<p align="center">Explore targets and dependencies, run builds and tests, and analyze build history.</p>

<p align="center">
  <a href="https://github.com/OhataKenji/zeal/actions/workflows/ci.yml"><img src="https://github.com/OhataKenji/zeal/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

[![Watch the Zeal demo on YouTube](docs/screenshots/overview.jpg)](https://youtu.be/Kkmc5_eOLA0)

<p align="center">
  <a href="https://youtu.be/Kkmc5_eOLA0">Watch on YouTube</a>
</p>

## Getting Started

> [!WARNING]
> Zeal is in development. Expect breaking changes. Local data may not carry over to new versions.

### Step1: Install

#### Option 1: Download a release

Prebuilt releases currently support **macOS (Apple Silicon) only**.

Download the app from [GitHub Releases](https://github.com/OhataKenji/zeal/releases).

#### Option 2: Build from source

Follow the [Development guide](docs/development.md#getting-started).

### Step2: Open a repository

**Requires:** Bazelisk or Bazel on your `PATH`.

On macOS and Linux, Zeal loads your login shell's `PATH`.

Open only trusted repositories.

1. Enter your Bazel workspace root path or select it with **Choose folder…**.
2. Keep **target patterns** as `//...` for all packages in the main repository. To narrow the scope, use a pattern such as `//app/...`.
3. Select **open**.
4. Select a target and run `build`, `test`, or `run`.

![Workspace form with an example path and target pattern](docs/screenshots/open-workspace.jpg)

## Features

### Workspace

Find the output directory or test logs without sifting through `bazel info`. Set which packages Zeal explores and copy the `.bazelrc` settings to record terminal builds alongside runs started in Zeal.

![Bazel Info for parcel-desk, including the output base, execution root, and test log directory](docs/screenshots/workspace-info.png)

### Targets

Browse packages or press `⌘K` / `Ctrl+K` to find a target by name, then build, test, or run it. Inspect its rule attributes in **Definition**, resolved outputs and dependencies in **Configured**, or each action's command, inputs, and outputs in **Actions**.

![The Actions tab for //rates:rate_server with CcStrip expanded to show its command, output, and inputs](docs/screenshots/target-actions.png)

### Modules

Find out why a Bzlmod dependency is in your build. Follow **Required by** back to its parents, or choose **Explain** to see why Bazel selected that version.

![The module inspector traces c-ares back to the root module through gRPC](docs/screenshots/module-explain.png)

### Query

Build a `deps` expression from blocks and see both the dependency graph and the Bazel command that produced it. Switch to **Code** to edit the expression directly, or use `cquery` and `aquery` to inspect configured targets and build actions.

![The visual query builder, its generated bazel query command, and the resulting graph of platform dependencies](docs/screenshots/query-graph.png)

### History

Follow live runs across worktrees and stop a build started in Zeal without switching back to its worktree. For finished runs, inspect logs and test results, select a span in **Timing** to see where the build spent its time, or use **Compare** to see which flags changed.

![Timing trace for a rate_server build with the nine-second analysis and execution phase selected on the main thread](docs/screenshots/history-timing.png)

### Analytics

Filter daily outcomes and p50/p95 durations for locally recorded builds and tests by workspace, command, or target pattern. The ranking shows which target patterns account for the most build time in total, regardless of the chart's target pattern filter.

![Daily invocation counts stacked by outcome, with workspace, command, and target pattern filters](docs/screenshots/analytics-invocations.png)

## Record terminal invocations

Zeal records commands run in the app. To also record terminal commands, add to `.bazelrc`:

```bazelrc
build --bes_backend=grpc://127.0.0.1:4718
build --experimental_build_event_upload_strategy=local
build --bes_timeout=5s
build --experimental_collect_system_network_usage
```

While Zeal is open, run `bazel build //...`.

If port 4718 is already in use, Zeal listens on a different port — open the
Workspace page for a one-off `--bes_backend` flag to pass to Bazel for that
session instead of editing `.bazelrc`.

## Development

- [Setup and checks](docs/development.md)

## License

[Apache-2.0](LICENSE)
