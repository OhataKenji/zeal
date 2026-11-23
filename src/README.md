# Frontend

`App.svelte` creates a session, calls `init` once on mount, and `dispose` on
unmount. `AppView.svelte` renders its view; `lib/session.ts` owns selection and
fetched data and calls IPC. Forms, Query, Analytics, comparison, and Console
keep their state in their views.

- Rust owns processes by workspace and invocation ID.
- History shows saved records and live executions across workspaces with the
  same Git origin. Console and Stop use the individual execution's owner.
  Without a BEP record, the owning workspace can show a bounded in-memory
  diagnostic (process exit code and output tail). Failed includes known process
  failures without implying a BEP result. A matching BEP record supersedes it.
- Async responses must match their captured owner and a live session, including
  A→B→A navigation. Core snapshots must have a newer revision.
- A build, test, or run waits for Zelith's outstanding target and workspace-info
  reads in that canonical workspace, including superseded reloads. Navigation
  or opening another workspace drops the automatic Console transition, not
  the submitted command. A rejected launch is reported with its original
  workspace, not as an invocation. Disposing the session cancels unsent
  starts. User invocations do not wait for other user invocations: Bazel
  still rejects output-base lock contention immediately. An explicit open
  blocks saved-workspace restoration even after failure.
- The target explorer builds one package tree per snapshot: the sidebar tree
  walks the nodes while `packageContents` projects a selection into the
  package table's direct targets, its `//path:all` heading and action
  pattern, and primary command — a scope note explains that Bazel may match
  unlisted targets. Selection and target reads live in the session;
  expansion stays local to the views.
  Run, Query, and the command palette share the target search index derived
  once per snapshot.
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
