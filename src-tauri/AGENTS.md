# src-tauri/AGENTS.md

Rust core of the Tauri app. Read the root [AGENTS.md](../AGENTS.md) first.

## Modules (`src/`)

- `label`: Bazel label helpers (canonicalization, package/name split, file path derivation).
- `xml`: streaming parse of `bazel query --output=xml` into raw rules.
- `snapshot`: builds the in-memory snapshot (targets, edges, files, meta) the UI renders.
- `state`: shared core state and `AppStateDto`, pushed to the UI as the `app-state` event.
- `workspace`: workspace detection, bazel executable resolution, persistence of the last opened workspace.
- `ingest`: runs `bazel query` (one at a time per workspace, coalescing requests) and swaps the snapshot.
- `lib.rs`: Tauri commands and app setup.

## Tests

- Golden fixtures live in `tests/fixtures/` (see the root AGENTS.md for how to generate them safely).

## DTOs

- DTOs sent to the UI serialize with camelCase field names (`#[serde(rename_all = "camelCase")]`).
- They are mirrored by hand in [src/lib/types.ts](../src/lib/types.ts); change both sides together.
