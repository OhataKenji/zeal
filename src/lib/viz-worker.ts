import { instance } from "@viz-js/viz";

// Graphviz layout can block for seconds on a large graph, so it runs off the
// UI thread here. The wasm compiles once per worker and stays warm between
// renders.
const viz = instance();

onmessage = async (e: MessageEvent<string>) => {
  try {
    const svg = (await viz).renderString(e.data, {
      format: "svg",
      graphAttributes: { bgcolor: "transparent", color: "#718494", fontcolor: "#e1e8ed" },
      nodeAttributes: { color: "#718494", fontcolor: "#e1e8ed", fontname: "Courier" },
      edgeAttributes: { color: "#718494", fontcolor: "#e1e8ed", fontname: "Courier" },
    });
    postMessage({ svg });
  } catch (error) {
    postMessage({ error: String(error) });
  }
};
