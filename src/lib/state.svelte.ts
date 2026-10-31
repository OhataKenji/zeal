import { COMMANDS } from "./commands.ts";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createSession, emptyView } from "./session";

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

function newSession() {
  return createSession(
    {
      invoke,
      listen: <T>(event: string, callback: (payload: T) => void) =>
        listen<T>(event, (e) => callback(e.payload)),
    },
    (next) => {
      view = next;
    },
  );
}

let session = newSession();

/** Returns the current IPC session used by application actions. */
export function getSession() {
  return session;
}

/** Replaces the session and starts event listening plus run-list refreshes. */
export function connect() {
  session.dispose();
  view = emptyView();
  session = newSession();
  void session.init();
  const current = session;
  const timer = setInterval(() => void current.loadRuns(), 1000);
  return () => {
    clearInterval(timer);
    current.dispose();
  };
}

/** Opens a workspace through the current session. */
export function openWorkspace(path: string) {
  return session.openWorkspace(path);
}

/** Requests a workspace refresh through the current session. */
export function refresh() {
  return session.refresh();
}

/** Opens a target location using the operating system's configured editor. */
export function openLocation(path: string): Promise<void> {
  return invoke(COMMANDS.openLocation, { path });
}
