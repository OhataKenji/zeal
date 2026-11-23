import type { TestContext } from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export async function createSvelteHarness(t: TestContext) {
  const cacheDir = await mkdtemp(join(tmpdir(), "zelith-svelte-"));
  let server: ViteDevServer | undefined;
  t.after(async () => {
    try {
      await server?.close();
    } finally {
      await rm(cacheDir, { recursive: true, force: true });
    }
  });
  server = await createServer({
    root: fileURLToPath(new URL("..", import.meta.url)),
    configFile: false,
    cacheDir,
    plugins: [svelte()],
    appType: "custom",
    logLevel: "silent",
    server: { middlewareMode: true, watch: null, ws: false },
    optimizeDeps: { noDiscovery: true, include: [] },
    ssr: { optimizeDeps: { exclude: ["svelte"] } },
  });
  const load = server.ssrLoadModule.bind(server);
  const { render } = await load("svelte/server");
  return { load, render };
}
