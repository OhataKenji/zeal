# Frontend

`App.svelte` creates one session, calls `init` once on mount, and calls `dispose`
on unmount. `AppView.svelte` renders its `SessionView` and delegates operations.
`lib/session.ts` owns selection and fetched data and calls IPC directly.
Forms, Query, Analytics, comparison, and Console keep their state in their views.

## Ownership and async work

- Rust owns running processes by workspace and invocation ID. UI selection does
  not change their owner or Stop target. A selected live Console becomes the
  same invocation's BEP result after release; background completion never navigates.
- Reads capture their workspace, selection, or reader object and reject stale
  responses by identity and disposal checks, including A→B→A navigation.
  Core snapshots also require a newer revision.
- Starts capture content and workspace-open identity. Target/package selection
  replaces content identity; explicit-open intent persists even after failure,
  preventing late replies or saved preferences from taking over selection.
- Target reads run detail → outputs → actions sequentially per selected
  workspace. A selection change takes over after the old request finishes;
  another workspace does not wait. History notifications during a read coalesce
  into one follow-up. Detail and profile reads remain independent.
- Query input changes clear the current result; execution is explicit.
  Disposing a view/session releases listeners and ignores late replies.

See [tests](../tests) for observable behavior and
[development](../docs/development.md) for setup and checks.
