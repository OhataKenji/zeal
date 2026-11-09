import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { AppState } from "../src/lib/types.ts";

const BASE: AppState = {
  besReady: false,
  scope: { include: ["//..."] },
  workspace: null,
  bazel: null,
  ingest: "idle",
  snapshotFetchedAt: null,
  lastError: null,
  activeInvocation: null,
  stopping: false,
  invocationNote: null,
  detailQuery: "idle",
};

const NOTICE =
  /Bazel or Bazelisk was not found on your PATH\. Please install either tool and add it to your PATH\./;

test("App shows missing Bazel instructions above the workspace picker", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-app-"));
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

    function output(state: AppState | null): string {
      Object.assign(app.view, { ready: true, state });
      return render(App)
        .body.replace(/<!--[\s\S]*?-->/g, "")
        .replace(/\s+/g, " ");
    }

    await t.test("a missing state shows loading without the notice or picker", () => {
      const html = output(null);
      assert.match(html, /class="content dim">loading…/);
      assert.doesNotMatch(html, NOTICE);
      assert.doesNotMatch(html, /Choose folder/);
      assert.doesNotMatch(html, /ws-prompt/);
    });

    await t.test("no workspace and no bazel shows the instructions above the picker", () => {
      const html = output({ ...BASE });
      assert.match(html, NOTICE);
      assert.match(html, /Choose folder/);
      assert.match(html, /ws-prompt/);
      assert.ok(
        html.indexOf("Bazel or Bazelisk was not found") < html.indexOf('class="ws-prompt"'),
      );
      assert.doesNotMatch(html, /restart Zeal/);
    });

    await t.test("a saved workspace without bazel keeps the workspace body", () => {
      const html = output({ ...BASE, workspace: "/workspace" });
      assert.doesNotMatch(html, NOTICE);
      assert.doesNotMatch(html, /Choose folder/);
      assert.doesNotMatch(html, /ws-prompt/);
      assert.match(html, /class="win-body"/);
    });

    await t.test("no workspace with a bazel executable shows the folder picker", () => {
      for (const bazel of ["/bin/bazel", "/bin/bazelisk"]) {
        const html = output({ ...BASE, bazel });
        assert.doesNotMatch(html, NOTICE);
        assert.doesNotMatch(html, /not found on PATH/);
        assert.match(html, /ws-prompt/);
        assert.match(html, /Choose folder…/);
      }
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
