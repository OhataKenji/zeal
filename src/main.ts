import { isTauri } from "@tauri-apps/api/core";
import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";

if (import.meta.env.DEV && !isTauri() && new URLSearchParams(location.search).has("preview")) {
  const { installPreview } = await import("./preview");
  installPreview();
}

// The entry HTML always includes the mount element.
mount(App, { target: document.getElementById("app-root")! });
