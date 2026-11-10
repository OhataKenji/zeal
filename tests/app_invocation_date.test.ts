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

    function output(startedAt: number | undefined): string {
      Object.assign(app.view, {
        ready: true,
        state: IDLE,
        invocations: [{ id: "invocation", cmd: "build", pattern: "//app:app", startedAt }],
      });
      return render(App).body;
    }

    for (const startedAt of [undefined, NaN, Infinity, -Infinity, 8.64e15 + 1]) {
      await t.test(`${startedAt} renders the missing-time fallback`, () => {
        const html = output(startedAt);
        assert.match(html, /Time not recorded/);
        assert.doesNotMatch(html, /<time\b/);
      });
    }

    for (const [startedAt, expected] of [
      [0, "1970-01-01T00:00:00.000Z"],
      [1_700_000_000_000, "2023-11-14T22:13:20.000Z"],
      [8_640_000_000_000_000, "+275760-09-13T00:00:00.000Z"],
    ] as const) {
      await t.test(`${startedAt} renders a semantic time`, () => {
        const html = output(startedAt);
        assert.ok(html.includes(`<time datetime="${expected}"`));
        assert.doesNotMatch(html, /Time not recorded/);
      });
    }
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
