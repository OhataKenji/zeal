import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppState } from "./types";

// The core pushes a full AppStateDto as the "app-state" event on every change;
// get_state provides the initial value. There is no other channel.
export const app = $state<{ state: AppState | null }>({ state: null });

export async function init(): Promise<void> {
  await listen<AppState>("app-state", (e) => {
    app.state = e.payload;
  });
  app.state = await invoke<AppState>("get_state");
}

export async function openWorkspace(path: string): Promise<void> {
  app.state = await invoke<AppState>("open_workspace", { path });
}

export function refresh(): Promise<void> {
  return invoke("refresh");
}

export function openLocation(path: string): Promise<void> {
  return invoke("open_location", { path });
}
