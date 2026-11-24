import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig, type Plugin } from "vite";
import { readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import sirv from "sirv";
import { fileURLToPath } from "node:url";

const PERFETTO_DIR = fileURLToPath(new URL("./src-tauri/resources/perfetto", import.meta.url));
const HISTORY_JSON = fileURLToPath(new URL("./src/preview/history.json", import.meta.url));

interface PreviewLane {
  tid: number;
  name: string;
  events: Record<string, unknown>[];
}
interface PreviewItem {
  record: { id: string };
  profile: { lanes: PreviewLane[] };
}

// Same-origin stand-in for the native `perfetto://` scheme: the browser-only
// `?preview` has no Tauri handler, so the dev server serves the pinned assets
// under this prefix plus a gzipped fixture trace per captured invocation id.
// The production build never runs this hook.
const VIEWER_CSP =
  "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; " +
  "style-src 'self' 'unsafe-inline'; worker-src 'self' blob:; " +
  "img-src 'self' data: blob:; font-src 'self' data:; " +
  "frame-ancestors 'self'; object-src 'none'; base-uri 'none'";

function previewTraces(): Map<string, unknown[]> {
  const history = JSON.parse(readFileSync(HISTORY_JSON, "utf8")) as PreviewItem[];
  // The capture stores thread ids on the lane; the trace format wants them
  // on every event plus a thread_name marker per lane.
  return new Map(
    history.map((item) => [
      item.record.id,
      item.profile.lanes.flatMap((lane) => [
        {
          ph: "M",
          name: "thread_name",
          pid: 1,
          tid: lane.tid,
          args: { name: lane.name },
        },
        ...lane.events.map((event) => ({ ...event, pid: 1, tid: lane.tid })),
      ]),
    ]),
  );
}

function perfettoPreview(): Plugin {
  return {
    name: "zelith-perfetto-preview",
    apply: "serve",
    configureServer(server) {
      const traces = previewTraces();
      server.middlewares.use("/__perfetto/trace", (req, res) => {
        const match = /^\/([^/]+)\/profile\.json\.gz$/.exec(req.url ?? "");
        const events = match ? traces.get(match[1]) : undefined;
        if (req.method !== "GET" || !events) {
          res.statusCode = 404;
          res.end();
          return;
        }
        res.statusCode = 200;
        res.setHeader("content-type", "application/octet-stream");
        res.setHeader("x-content-type-options", "nosniff");
        res.end(gzipSync(JSON.stringify({ traceEvents: events })));
      });
      server.middlewares.use(
        "/__perfetto",
        sirv(PERFETTO_DIR, {
          dev: true,
          setHeaders(res) {
            res.setHeader("content-security-policy", VIEWER_CSP);
          },
        }),
      );
    },
  };
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte(), perfettoPreview()],
  // Pre-bundle lazy Console dependencies so its first open does not reload the app.
  optimizeDeps: {
    include: ["@xterm/addon-fit", "@xterm/xterm"],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
  // 3. to access the Tauri environment variables set by the CLI with type safety,
  // add the `TAURI_` prefix to any variables starting with `TAURI_`
  envPrefix: ["VITE_", "TAURI_ENV_*"],
});
