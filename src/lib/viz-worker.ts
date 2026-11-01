import { instance } from "@viz-js/viz";

// Graphviz layout can block for seconds on a large graph, so it runs off the
// UI thread here. The wasm compiles once per worker and stays warm between
// renders.
const viz = instance();

onmessage = async (e: MessageEvent<string>) => {
  try {
    const svg = (await viz).renderString(e.data, { format: "svg" });
    postMessage({ svg });
  } catch (error) {
    postMessage({ error: String(error) });
  }
};
