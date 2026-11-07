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
  query: "idle",
  detailQuery: "idle",
};

test("QueryView reflects the target detail query's Bazel slot", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-query-view-"));
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
    const { bazelBusyReason } = await server.ssrLoadModule("/src/lib/session/session.ts");
    const { default: QueryView } = await server.ssrLoadModule("/src/lib/query/QueryView.svelte");
    const { render } = await server.ssrLoadModule("svelte/server");

    function output(detailQuery: AppState["detailQuery"]): string {
      return render(QueryView, {
        props: {
          snap: null,
          busyReason: bazelBusyReason({ ...IDLE, detailQuery }),
        },
      }).body;
    }

    await t.test("a running detail query disables Execute and explains why", () => {
      const html = output("running");
      assert.match(html, /class="button button-primary"[^>]*disabled/);
      assert.match(html, /title="target details are loading"/);
      assert.match(html, /class="query-busy dim"[^>]*>\s*target details are loading\s*</);
    });

    await t.test("an idle detail query leaves Execute enabled with no reason", () => {
      const html = output("idle");
      assert.match(html, /class="button button-primary" title="Execute query"/);
      assert.doesNotMatch(html, /class="button button-primary"[^>]*disabled/);
      assert.doesNotMatch(html, /query-busy/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
