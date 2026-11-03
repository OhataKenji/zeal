<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Zeal logo" width="128">
</p>

<h1 align="center">Zeal</h1>

<p align="center">A local-first Bazel GUI client.</p>

<p align="center">Explore targets and dependencies, run builds and tests, and analyze build history.</p>

<p align="center">
  <a href="https://github.com/OhataKenji/zeal/actions/workflows/ci.yml"><img src="https://github.com/OhataKenji/zeal/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

![Zeal target explorer showing source files, dependencies, and invocation history](docs/screenshots/overview.jpg)

## Features

### Find and Run Targets

Find targets with fuzzy search. Run `build`, `test`, or `run`.

![Fuzzy search for Bazel targets](docs/screenshots/fuzzy-search.jpg)

### Invocation Page

View logs, results, details, and available timing data.

![Invocation Page showing build phases and a per-thread timing trace](docs/screenshots/run-page.jpg)

### Analytics Page

Filter build and test history from Zeal and recorded terminal commands. Compare counts, results, and durations (total, p50, p95).

![Analytics Page showing date filters and daily invocation counts by outcome](docs/screenshots/analytics-page.jpg)

## Getting Started

> [!WARNING]
> Zeal is in development. Expect breaking changes. Local data may not carry over to new versions.

Run Zeal from source: [Development guide](docs/development.md#getting-started).

### Open a repository

**Requires:** Bazelisk or Bazel on your `PATH`.

Open only trusted repositories.

1. Enter the path to your Bazel workspace root in **Workspace**.
2. Keep **target patterns** as `//...` for all packages in the main repository. To narrow the scope, use a pattern such as `//app/...`.
3. Select **open**.
4. Select a target and run `build`, `test`, or `run`.

![Workspace form with an example path and target pattern](docs/screenshots/open-workspace.jpg)

## Usage

On macOS and Linux, Zeal loads your login shell's `PATH`.

### Record terminal invocations

Add to `.bazelrc`:

```bazelrc
build --bes_backend=grpc://127.0.0.1:4718
build --bes_upload_mode=wait_for_upload_complete
build --bes_timeout=5s
```

While Zeal is open, run `bazel build //...`.

Zeal and terminal runs save profiles through the same receiver. It copies Bazel's
standard gzip profile from the same machine. Profiling follows your Bazel
configuration. Unavailable profiles do not prevent build history from being saved.

## Development

- [Setup and checks](docs/development.md)

## License

[Apache-2.0](LICENSE)
