import type { TargetScope } from "./types.ts";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createSession, emptyView, type WorkspacePage } from "./session";

// Raw profiles avoid deep proxying. The session replaces the view; timeline
// geometry is derived separately from the original event fields.
let view = $state.raw(emptyView());
/** Reactive application state shared by the mounted Svelte views. */
export const app = {
  get view() {
    return view;
  },
  get state() {
    return view.state;
  },
};

function newSession(page: WorkspacePage = "targets") {
  return createSession(
    {
      invoke,
      listen: <T>(event: string, callback: (payload: T) => void) =>
        listen<T>(event, (e) => callback(e.payload)),
    },
    (next) => {
      view = next;
    },
    page,
  );
}

let session = newSession();

/** Returns the current IPC session used by application actions. */
export function getSession() {
  return session;
}

/** Replaces the session and starts event listening. */
export function connect(page: WorkspacePage = "targets") {
  session.dispose();
  view = emptyView(page);
  session = newSession(page);
  void session.init();
  const current = session;
  return () => {
    current.dispose();
  };
}

/** Opens a workspace through the current session. */
export function openWorkspace(path: string, scope: TargetScope) {
  return session.openWorkspace(path, scope);
}

/** Requests a workspace refresh through the current session. */
export function refresh() {
  return session.refresh();
}
