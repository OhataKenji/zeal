import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createSession, emptyView } from "./session";

// Raw profiles avoid deep proxying. The session replaces the view; timeline
// geometry is derived separately from the original event fields.
let view = $state.raw(emptyView());
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

export let session = newSession();

export function connect() {
  session.dispose();
  view = emptyView();
  session = newSession();
  void session.init();
  return session.dispose;
}

export const openWorkspace = (path: string) => session.openWorkspace(path);
export const refresh = () => session.refresh();

export function openLocation(path: string): Promise<void> {
  return invoke("open_location", { path });
}
