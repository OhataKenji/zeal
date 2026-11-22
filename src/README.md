# Frontend

`App.svelte` creates a session, calls `init` once on mount, and `dispose` on
unmount. `AppView.svelte` renders its view; `lib/session.ts` owns selection and
fetched data and calls IPC. Forms, Query, Analytics, comparison, and Console
keep their state in their views.

- Rust owns processes by workspace and invocation ID. A selected live Console
  becomes the same invocation's BEP result after release; other selections stay put.
- Async responses must match their captured owner and a live session, including
  A→B→A navigation. Core snapshots must have a newer revision.
- Starts capture content and workspace-open identity. A target/package change
  invalidates the captured content; explicit-open intent blocks saved-workspace
  restoration even after failure.
- Target reads have two lifetimes: static detail once per selection, then one
  phase reader for the active inspector tab (configured or actions) under the
  workspace's applied build flags; Definition issues no phase read. The UI
  stages the Configured tab's three `read_target` steps — configuration,
  transitions, outputs — each a `config(label, target)` cquery scoped to the
  selected target's own configuration, so a tab, flag, or
  selection change can stop between them; each step is one Bazel process and
  `--noblock_for_lock` applies per process, never across them. Applied
  flags persist verbatim; `read_target` rejects malformed or reserved
  flags before spawning Bazel and the diagnostic surfaces as the phase
  error. Tab and flag changes replace the phase reader, so
  stale replies cannot land. Other workspaces do not wait. History notifications
  coalesce into one follow-up. Saved detail/profile reads remain independent.
- The selected workspace keeps the module graph, selection, and explanation across
  page navigation. Switching workspaces discards them. Search and depth filters
  are local to the Modules page and reset when you leave it.
- Query input changes clear results; execution is explicit.

See [tests](../tests) for behavior and [development](../docs/development.md) for checks.
