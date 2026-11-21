# Frontend

`App.svelte` creates a session, calls `init` once on mount, and `dispose` on
unmount. `AppView.svelte` renders its view; `lib/session.ts` owns selection and
fetched data and calls IPC. Forms, Modules, Query, Analytics, comparison, and Console
keep their state in their views.

- Rust owns processes by workspace and invocation ID. A selected live Console
  becomes the same invocation's BEP result after release; other selections stay put.
- Async responses must match their captured owner and a live session, including
  A→B→A navigation. Core snapshots must have a newer revision.
- Starts capture content and workspace-open identity. A target/package change
  invalidates the captured content; explicit-open intent blocks saved-workspace
  restoration even after failure.
- Target reads run detail → outputs → actions in order per selected workspace;
  other workspaces do not wait. History notifications coalesce into one follow-up.
  Saved detail/profile reads remain independent.
- Query input changes clear results; execution is explicit.

See [tests](../tests) for behavior and [development](../docs/development.md) for checks.
