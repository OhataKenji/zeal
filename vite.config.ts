import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    svelte(),
    {
      name: "worker-csp",
      configureServer(server) {
        server.middlewares.use((request, response, next) => {
          const url = new URL(request.url ?? "/", "http://localhost");
          const destination = request.headers["sec-fetch-dest"];
          if (
            url.searchParams.has("worker_file") ||
            destination === "worker" ||
            destination === "sharedworker" ||
            destination === "serviceworker"
          ) {
            // Workers do not inherit the document's CSP. Development needs
            // same-origin module imports; the bundled worker needs only Wasm.
            response.setHeader(
              "Content-Security-Policy",
              "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'none'",
            );
          }
          next();
        });
      },
    },
  ],

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
