# src/AGENTS.md

Svelte 5 + TypeScript UI. Read the root [AGENTS.md](../AGENTS.md) first.

- Shared app state lives in `lib/state.svelte.ts`, updated by the core through the `app-state` event and the `get_state` command. Run logs and run details use separate events and commands as defined in [M2](../docs/design/m2-run-detail.md); do not route them through shared app state.
- Use Svelte 5 runes (`$state`, `$derived`, `$effect`, `$props`).
- Types shared with the core are in `lib/types.ts`, kept in sync by hand with the Rust DTOs (see [src-tauri/AGENTS.md](../src-tauri/AGENTS.md)).
