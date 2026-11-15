# Frontend

Svelte renders the UI. Session state follows an Elm-like loop: a Message enters
`update`, which returns the next Model and Effects. The runner executes each
Effect and returns a result Message to the same loop.

## Code map

| File                                         | Responsibility                                                           |
| -------------------------------------------- | ------------------------------------------------------------------------ |
| [App.svelte](App.svelte)                     | Render the current content and send user actions.                        |
| [lib/state.svelte.ts](lib/state.svelte.ts)   | Connect the session to Svelte's reactive view and Tauri transport.       |
| [lib/session_model.ts](lib/session_model.ts) | Model, Message, Effect, pure `update`, and derived loading state.        |
| [lib/session.ts](lib/session.ts)             | Dispatch messages, publish the view, execute IPC, and own subscriptions. |

For example, selecting a target sends one `select` Message. `update` switches
content to targets, clears the saved invocation selection, and requests target
detail when eligible. The runner returns `targetDetailResult`; `update` accepts
the result and requests outputs. The UI never has to close Console first.

## State ownership

- The Model groups state by ownership: `connection`, `workspace`, `navigation`,
  `workspacePrompt`, `startRequest`, `targets`, `history`, `saved`, and
  `nextRequestId`. `SessionView` is a flat projection, not stored state.
- `connection.state` is the aggregate `AppState`: registered workspaces plus
  every owned invocation, delivered as snapshots guarded by the backend
  `revision`. `workspace` is the
  selected canonical id; `navigation` is an epoch bumped by explicit navigation
  so delayed start replies cannot reroute the view. `startRequest` is the one
  foreground launch awaiting its reply; switching workspaces clears it while
  the native execution stays owned by the core.
- `content` is the current destination: targets, invocation, Analytics, or
  Query. The invocation destination carries the workspace page to return to;
  workspace-page destinations are authoritative on their own.
- Target selection is independent of visibility. The Console component mounts
  only while the selected invocation has a live owned entry; once capture
  finishes the core releases the entry and the same destination shows the
  saved BEP result instead.
- Form input, focus, expanded panels, and the native folder picker stay local to
  Svelte. Console byte streams and terminal handles also stay outside the Model.

## Asynchronous results

Each Reader stores `value`, `error`, `running`, and `wanted`. A refresh failure
can leave the previous value alongside an error. `running` is the in-flight
request ID; `wanted` is the request ID whose result is still needed. Changing
selection invalidates the wanted result, not the IPC operation. An old response
releases `running` and starts the latest wanted request, but cannot restore
obsolete data.

History keeps one request with status `current`, `refresh`, or `invalidated`.
Repeated notifications coalesce into one follow-up; a workspace change
invalidates the in-flight result and still releases the worker when it returns.

`update` never performs IPC. `runEffect` returns `Promise<SessionMessage>` and
converts IPC failures to result Messages; it does not decide whether results are
current or change navigation. Every Effect carries the workspace captured when
it was requested, never the live selection. Event subscriptions send Messages
independently. Disposal stops dispatching results, not already-started IPC.

See the [session design](../docs/design/session-state.md) for rationale and
ordering rules, [pure transition tests](../tests/session_model.test.ts) for
Model/Message examples, and [session tests](../tests/session.test.ts) for transport
and notification ordering. Setup and checks are in the
[development guide](../docs/development.md).
