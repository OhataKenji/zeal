# parcel-desk

A minimal Bazel workspace for a parcel-rating service: a pure C++ tariff
library (`//rates:tariff`), a synchronous gRPC server (`//rates:rate_server`)
and a Python client/CLI (`//dispatch:quote`) sharing generated stubs
(`//api:python`). `//dispatch:quote_contract_test` starts the built server on
an ephemeral loopback port and verifies real quotes over gRPC.

## Prerequisites

- Bazelisk or Bazel on `PATH` (`.bazelversion` pins Bazel 7.6.1).
- A C++ toolchain (Xcode Command Line Tools on macOS).
- Network access for the first build: Bazel fetches gRPC 1.71.0, protobuf,
  rules_python and a standalone Python 3.12 interpreter, then compiles the
  C++ and Python gRPC runtimes from source. Subsequent builds are incremental.

## Domain

`api/rates.proto` defines `dispatch.rates.v1`:

- `Zone` — `LOCAL`, `REGIONAL`, `REMOTE` (plus `ZONE_UNSPECIFIED`).
- `Parcel` — a zone and a weight in grams.
- `Quote` — `price_cents` and `transit_days`.
- `RateDesk.QuoteParcel` — the only RPC.

`rates/tariff.{h,cc}` is the pure domain core:
`std::optional<Estimate> QuoteRate(Zone zone, uint32_t grams)`. It returns
`std::nullopt` for an unspecified/unknown zone or a weight of zero or over
30,000 grams. Otherwise it prices a zone base rate (LOCAL 450 ¢/1 day,
REGIONAL 650 ¢/2 days, REMOTE 900 ¢/4 days) plus 75 ¢ for every additional
started 500 g block: `(grams - 1) / 500`.

## Run

```sh
bazel build //rates:rate_server

# Terminal 1: prints "listening on 127.0.0.1:50051". The only argument is a
# port; `0` picks an ephemeral one. The server always binds 127.0.0.1.
bazel run //rates:rate_server

# Terminal 2
bazel run //dispatch:quote -- --zone regional --grams 1250
# -> REGIONAL 1250 g: $8.00, 2 business days
```

```sh
bazel test //dispatch:quote_contract_test
```

To send build events to a local Zeal listener while Zeal is open:

```sh
bazel build //rates:rate_server \
  --bes_backend=grpc://127.0.0.1:4718 \
  --experimental_build_event_upload_strategy=local \
  --bes_timeout=5s
```

## Explore in Zeal

The preview fixtures are captured from this workspace. In the Targets page,
follow the dependency chain from the dispatch layer down to the proto and
the tariff core: select `//dispatch:quote_contract_test` (its `data` dep is
`//rates:rate_server`), then `//rates:rate_server` (deps `//rates:tariff`
and `//api:rates_cc_grpc`), then `//rates:tariff` (deps
`//api:rates_cc_proto`, which generates `Zone` from `api/rates.proto`).

## Layout

- `api/rates.proto` — the service definition (`dispatch.rates.v1`).
- `api/BUILD` — `proto_library` plus gRPC's `cc_proto_library`,
  `cc_grpc_library`, `py_proto_library` and `py_grpc_library` rules (loaded
  from `@com_github_grpc_grpc//bazel:...`, the same rules grpc's own
  `examples/protos/BUILD` uses).
- `rates/tariff.{h,cc}` — `QuoteRate`, the pure rating rules.
- `rates/rate_server.cc` — sync `RateDesk` implementation; invalid parcels
  get `INVALID_ARGUMENT`; binds 127.0.0.1 only and takes an optional port
  argument.
- `dispatch/quote.py` — `quote(address, zone, grams)` plus the `quote` CLI.
- `dispatch/quote_contract_test.py` — `py_test` that runs
  `//rates:rate_server` from runfiles with port `0`, reads the ephemeral
  port it prints, and asserts real quotes for LOCAL/REGIONAL/REMOTE plus
  `INVALID_ARGUMENT` for invalid parcels.
