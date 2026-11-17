# Frontend

Svelte renders the UI. `App.svelte` owns the session's lifetime: it builds the
Tauri transport, creates the session, feeds each published `SessionView` to
`AppView`, and disposes the session on unmount. `lib/session.ts` owns the
session's mutable state and calls workspace, target, and saved-invocation IPC directly: each async
operation captures the workspace, target selection, or reader it belongs to,
and a `current()` predicate rejects late replies that no longer match.

## Code map

| File                                                     | Responsibility                                                                       |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| [App.svelte](App.svelte)                                 | Own the session lifetime and pass `view`/`session` down.                             |
| [AppView.svelte](AppView.svelte)                         | Render the current content and send user actions.                                    |
| [lib/session.ts](lib/session.ts)                         | Private session state, stale guards, IPC reads, subscriptions, and the derived view. |
| [lib/WorkspacePrompt.svelte](lib/WorkspacePrompt.svelte) | Workspace open/settings form; its input and in-flight flag stay local.               |

Query, Analytics, and Console own their view-scoped controllers and call their
IPC operations separately.

For example, selecting a target calls `select`, which replaces the workspace's
target context and runs one detail -> outputs -> actions chain. The UI never
has to close Console first.

## State ownership

- `SessionState` groups state by ownership: `core` (the revisioned `AppState`
  aggregate), `workspace` (snapshot, target context, history), `content` (the
  displayed destination), `navigation` (an epoch bumped by explicit navigation),
  `start` (the one foreground launch awaiting its reply), `error`, and `saved`
  (the selected invocation's detail/profile readers). `SessionView` is a flat
  projection, not stored state. The latest workspace-open request is tracked by
  a `workspaceRequest` token inside the session, not in the view.
- `core` carries registered workspaces plus every owned invocation, guarded by
  the backend `revision`; older or equal pushes are ignored.
- `content` is the current destination: targets, invocation, Analytics, or
  Query. The invocation destination carries the workspace page to return to.
- Target selection is independent of visibility. The Console component mounts
  only while the selected invocation has a live owned entry; once capture
  finishes the core releases the entry and the same destination shows the
  saved BEP result instead.
- Form input, focus, expanded panels, the workspace-open/settings dialog, and
  the native folder picker stay local to Svelte. Console byte streams and
  terminal handles also stay outside the session state.

## Asynchronous results

Each reader is `{ value, error, loading }`; a failed refresh can leave the
previous value alongside an error. Every async operation captures the object
it belongs to — the `Workspace`, the `TargetSelection`, the `Saved` readers,
the refresh token, the start token, or the workspace-open token — and `current()`
re-checks that identity plus `disposed` before writing. A stale reply may
finish but cannot publish, overwrite current values, or continue obsolete
target work; its completion can still start the latest selection's chain.
A->B->A invalidates the first A as well.

Target reads form one sequential chain per displayed workspace
(detail -> outputs -> actions) because those RPCs share the Bazel
output_base. Only the latest selection proceeds when the previous chain
finishes; outputs may fail while actions still load, and a detail failure
suppresses both.

History keeps one request per workspace. Notifications received while a read
is in flight coalesce into a single follow-up; owned-state-only pushes do not
refetch history. A history update re-reads the selected invocation's saved
detail/profile when the record changed or no longer matches the cached detail.

`init` registers the `invocations-changed` then `app-state` listeners before
the initial `get_state`, shares concurrent calls, and `dispose` removes the
listeners and makes every later callback and reply a no-op. A listener that
finishes registering during disposal is removed immediately, and a failed
second registration cleans up the first.

See the [session design](../docs/design/session-state.md) for rationale and
ordering rules and the [session tests](../tests/session.test.ts) for transport
and race coverage. Setup and checks are in the
[development guide](../docs/development.md).
