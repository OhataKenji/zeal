import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig, type Plugin } from "vite";
import sirv from "sirv";
import { fileURLToPath } from "node:url";

const PERFETTO_DIR = fileURLToPath(new URL("./src-tauri/resources/perfetto", import.meta.url));

// Same-origin stand-in for the native loopback viewer: the browser-only
// `?preview` has no Tauri server, so the dev server mounts the pinned assets
// under this prefix. The production build never runs this hook.
const VIEWER_CSP =
  "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; " +
  "style-src 'self' 'unsafe-inline'; worker-src 'self' blob:; " +
  "img-src 'self' data: blob:; font-src 'self' data:; " +
  "frame-ancestors 'self'; object-src 'none'; base-uri 'none'";

function perfettoPreview(): Plugin {
  return {
    name: "zelith-perfetto-preview",
    apply: "serve",
    configureServer(server) {
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
