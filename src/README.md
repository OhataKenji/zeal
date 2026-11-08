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

- `content` is the current destination: targets, invocation, Console, Analytics,
  or Query. The invocation ID belongs to that destination, not a separate flag.
- Target selection and the retained Console record are independent of visibility.
  Hiding Console does not unmount its terminal. `workspacePage` remembers the
  workspace page to show after changing workspace, even when an invocation was
  displayed in between; it is not another visibility flag.
- Form input, focus, expanded panels, and the native folder picker stay local to
  Svelte. Console byte streams and terminal handles also stay outside the Model.

## Asynchronous results

Each reader has `running` (the in-flight request ID) and `wanted` (the request ID
whose result is still needed). Changing selection invalidates the wanted result,
not the IPC operation. An old response releases `running` and starts the latest
wanted request, but cannot restore obsolete data.

`update` never performs IPC. `runEffect` returns `Promise<SessionMessage>` and
converts IPC failures to result Messages; it does not decide whether results are
current or change navigation. Event subscriptions send Messages independently.
Disposal stops dispatching results, not already-started IPC.

See the [session design](../docs/design/session-state.md) for rationale and
ordering rules, [pure transition tests](../tests/session_model.test.ts) for
Model/Message examples, and [session tests](../tests/session.test.ts) for transport
and notification ordering. Setup and checks are in the
[development guide](../docs/development.md).
