import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { emptyAnalytics, type AnalyticsView as AnalyticsState } from "../src/lib/analytics.ts";

test("AnalyticsView offers a workspace selector defaulting to all workspaces", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zelith-analytics-view-"));
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
    const { default: AnalyticsView } = await server.ssrLoadModule("/src/lib/AnalyticsView.svelte");
    const { render } = await server.ssrLoadModule("svelte/server");

    function output(view: AnalyticsState = emptyAnalytics()): string {
      return render(AnalyticsView, {
        props: { view, onload: () => {}, onbreakdown: () => {} },
      }).body.replace(/<!--[\s\S]*?-->/g, "");
    }

    function tag(html: string, pattern: RegExp): string {
      const match = html.match(pattern);
      assert.ok(match, `missing element matching ${pattern}`);
      return match[0];
    }

    await t.test("the selector is labeled and defaults to every workspace", () => {
      const html = output();
      assert.match(html, /<label[^>]*>\s*Workspace\s*<select[^>]*aria-label="Workspace path"/);
      const select = tag(html, /<select[^>]*aria-label="Workspace path"[\s\S]*?<\/select>/);
      const option = tag(select, /<option[^>]*>All workspaces<\/option>/);
      assert.match(option, /value=""/);
      assert.match(option, /selected=""/);
      assert.equal(select.match(/<option/g)?.length, 1);
    });

    await t.test("renders persisted filter and breakdown state from session view", () => {
      const html = output({
        ...emptyAnalytics(),
        filter: {
          start: "2026-03-01",
          end: "2026-03-15",
          cmd: "test",
          outcome: "failed",
          pattern: "//app:server",
          workspaceId: null,
        },
        breakdown: "workspace",
      });
      assert.match(html, /aria-label="Start date"[^>]*value="2026-03-01"/);
      assert.match(html, /aria-label="End date"[^>]*value="2026-03-15"/);
      assert.match(html, /aria-label="Command"[^>]*value="test"/);
      assert.match(html, /aria-label="Target patterns"[^>]*value="\/\/app:server"/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
