import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { Section } from "../src/lib/types.ts";

test("LogPanel displays the available log and its section status", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-log-panel-"));
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
    const { default: LogPanel } = await server.ssrLoadModule("/src/lib/run/LogPanel.svelte");
    const { render } = await server.ssrLoadModule("svelte/server");

    function output(section: Section<string> | undefined, loading = false): string {
      return render(LogPanel, { props: { section, loading } }).body;
    }

    await t.test("incomplete logs show the saved prefix and the missing-tail notice", () => {
      const html = output({
        status: "incomplete",
        data: "saved prefix",
        note: "event tail is missing",
      });
      assert.match(html, /incomplete — event tail is missing/);
      assert.match(html, /saved prefix/);
      assert.match(html, /class="notice notice--warning" role="status"/);
      assert.match(html, /aria-label="Build stdout and stderr"/);
    });

    await t.test("an empty incomplete log still explains the missing data", () => {
      const html = output({ status: "incomplete", data: "", note: "invalid last event" });
      assert.match(html, /incomplete — invalid last event/);
      assert.match(html, /aria-label="Build stdout and stderr"/);
    });

    await t.test("complete logs have no warning", () => {
      const html = output({ status: "ok", data: "complete output" });
      assert.match(html, /complete output/);
      assert.doesNotMatch(html, /class="notice|Loading build log/);
    });

    await t.test("an outstanding log request shows the existing loading message", () => {
      const html = output(undefined, true);
      assert.match(html, /Loading build log…/);
      assert.doesNotMatch(html, /class="notice|aria-label="Build stdout and stderr"/);
    });

    await t.test("an unreadable log shows its error without inventing log content", () => {
      const html = output({ status: "read_error", note: "permission denied" });
      assert.match(html, /read error — permission denied/);
      assert.match(html, /class="notice notice--error" role="alert"/);
      assert.doesNotMatch(html, /aria-label="Build stdout and stderr"|Loading build log/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
