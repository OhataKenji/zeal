import { isTauri } from "@tauri-apps/api/core";
import { mount } from "svelte";
import "./theme.css";
import "./app.css";
import App from "./App.svelte";

if (import.meta.env.DEV && !isTauri() && new URLSearchParams(location.search).has("preview")) {
  const { installPreview } = await import("./preview");
  installPreview();
}

const app = mount(App, {
  target: document.getElementById("app")!,
});

export default app;
