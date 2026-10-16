# src/AGENTS.md

Svelte 5 + TypeScript UI. Read the root [AGENTS.md](../AGENTS.md) first.

- App state lives in `lib/state.svelte.ts`. Its only sources are the `app-state` event pushed by the core and the `get_state` command; do not add other state channels.
- Use Svelte 5 runes (`$state`, `$derived`, `$effect`, `$props`).
- Types shared with the core are in `lib/types.ts`, kept in sync by hand with the Rust DTOs (see [src-tauri/AGENTS.md](../src-tauri/AGENTS.md)).
