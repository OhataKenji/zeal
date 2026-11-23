# Frontend

`App.svelte` creates a session, calls `init` once on mount, and `dispose` on
unmount. `AppView.svelte` renders its view; `lib/session.ts` owns selection and
fetched data and calls IPC. Forms, Query, Analytics, comparison, and Console
keep their state in their views.

- Rust owns processes by workspace and invocation ID.
- Console serves live terminal output; History is workspace-scoped and shows a
  saved BEP record or, for a terminal completion without one, a bounded
  in-memory process diagnostic (process exit code and output tail) that a
  later matching BEP record supersedes.
- Async responses must match their captured owner and a live session, including
  A→B→A navigation. Core snapshots must have a newer revision.
- A build, test, or run waits for Zeal's outstanding target and workspace-info
  reads in that canonical workspace, including superseded reloads. Navigation
  or opening another workspace drops the automatic Console transition, not
  the submitted command. Disposing the session cancels unsent starts. User invocations do
  not wait for other user invocations: Bazel still rejects output-base lock
  contention immediately. An explicit open blocks saved-workspace restoration
  even after failure.
- The target explorer builds one package tree per snapshot: the sidebar tree
  and the package table read the same nodes. Selection and target reads live
  in the session; expansion and direct/subtree scope stay local to the views.
- Target reads have two lifetimes: static detail once per selection, then one
  phase reader for the active target detail tab (configured or actions) under the
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
