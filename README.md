# Zeal

A local first Bazel GUI client.

![Zeal target explorer showing source files, dependencies, and invocation history](docs/screenshots/overview.jpg)

## Features

### Find a Target and Invoke It

Find Bazel targets by fuzzy-searching their full labels, then choose `build`, `test`, or `run`.

### Invocation Page

Inspect an invocation's logs, target results, details, and per-thread timing trace from the history.

![Invocation Page showing build phases and a per-thread timing trace](docs/screenshots/run-page.jpg)

### Analytics Page

Filter local build and test history from the GUI and recorded terminal invocations to compare invocation counts, outcomes, total invocation time, and p50/p95 durations.

![Analytics Page showing date filters and daily invocation counts by outcome](docs/screenshots/analytics-page.jpg)

## Usage

On macOS and Linux, Zeal loads your login shell's `PATH` at startup.

### Record terminal invocations

Add to `.bazelrc` and use `bazel build --config=zeal //...` while Zeal is open:

```bazelrc
build:zeal --bes_backend=grpc://127.0.0.1:4718
build:zeal --bes_timeout=5s
```

## Development

See the [development guide](docs/development.md) for setup and checks, the [release guide](docs/release-guide.md) for macOS releases, and [the PRD](docs/prd.md) and [design documents](docs/design/) for more details.

This project is currently maintained by its authors. We are not accepting external issues or pull requests at this time.

## License

[Apache-2.0](LICENSE)
