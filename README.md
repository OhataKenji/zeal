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

## Getting Started

> [!WARNING]
> Zeal is in development. Expect breaking changes. Local data may not carry over to new versions.

Run Zeal from source: [Development guide](docs/development.md#getting-started).

### Open a repository

**Requires:** Bazelisk or Bazel on your `PATH`.

Open only trusted repositories.

1. Enter your Bazel workspace path in **Workspace** and select **open**.
2. Find a target and run `build`, `test`, or `run`.

![Workspace form with an example path and target pattern](docs/screenshots/open-workspace.jpg)

Zeal records commands run in the app. To also record terminal builds,
[connect Bazel to Zeal](docs/terminal-builds.md).

## Features

### Find and Run Targets

Find targets with fuzzy search. Run `build`, `test`, or `run`.

![Fuzzy search for Bazel targets](docs/screenshots/fuzzy-search.jpg)

### Invocation Page

View logs, results, details, and available timing data.

![Invocation Page showing build phases and a per-thread timing trace](docs/screenshots/run-page.jpg)

### Analytics Page

See build trends and compare durations.

![Analytics Page showing date filters and daily invocation counts by outcome](docs/screenshots/analytics-page.jpg)

## Development

- [Setup and checks](docs/development.md)

## License

[Apache-2.0](LICENSE)
