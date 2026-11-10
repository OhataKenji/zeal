import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { AppState } from "../src/lib/types.ts";

const IDLE: AppState = {
  besReady: false,
  scope: { include: ["//app/..."] },
  workspace: "/workspace",
  bazel: "/bin/bazel",
  ingest: "idle",
  snapshotFetchedAt: null,
  lastError: null,
  activeInvocation: null,
  stopping: false,
  invocationNote: null,
  detailQuery: "idle",
};

test("App renders invocation dates safely", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-app-invocation-date-"));
  let server: ViteDevServer | undefined;
  try {
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
    const { app } = await server.ssrLoadModule("/src/lib/state.svelte.ts");
    const { default: App } = await server.ssrLoadModule("/src/App.svelte");
    const { render } = await server.ssrLoadModule("svelte/server");

    function invocationRow(startedAt: number | undefined): string {
      Object.assign(app.view, {
        ready: true,
        state: IDLE,
        invocations: [{ id: "invocation", cmd: "build", pattern: "//app:app", startedAt }],
      });
      const html = render(App).body;
      const row = html.match(
        /<button\b[^>]*class="[^"]*\binvocation-row\b[^"]*"[^>]*>[\s\S]*?<\/button>/,
      )?.[0];
      assert.ok(row, "invocation row is rendered");
      return row;
    }

    for (const startedAt of [undefined, NaN, Infinity, -Infinity, 8.64e15 + 1]) {
      await t.test(`${startedAt} renders the missing-time fallback`, () => {
        const row = invocationRow(startedAt);
        assert.match(row, /Time not recorded/);
        assert.doesNotMatch(row, /<time\b/);
      });
    }

    for (const startedAt of [0, 1_700_000_000_000, 8_640_000_000_000_000]) {
      await t.test(`${startedAt} renders a semantic time`, () => {
        const row = invocationRow(startedAt);
        const date = new Date(startedAt);
        assert.ok(row.includes(`<time datetime="${date.toISOString()}"`));
        assert.doesNotMatch(row, /Time not recorded/);
      });
    }
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
