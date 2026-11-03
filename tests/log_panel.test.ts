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
    const { default: LogPanel } = await server.ssrLoadModule("/src/lib/invocation/LogPanel.svelte");
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
      assert.match(html, /aria-label="Invocation output"/);
    });

    await t.test("an empty incomplete log still explains the missing data", () => {
      const html = output({ status: "incomplete", data: "", note: "invalid last event" });
      assert.match(html, /incomplete — invalid last event/);
      assert.match(html, /aria-label="Invocation output"/);
    });

    await t.test("complete logs have no warning", () => {
      const html = output({ status: "ok", data: "complete output" });
      assert.match(html, /complete output/);
      assert.doesNotMatch(html, /class="notice|Loading invocation output/);
    });

    await t.test("an outstanding log request shows the existing loading message", () => {
      const html = output(undefined, true);
      assert.match(html, /Loading invocation output…/);
      assert.doesNotMatch(html, /class="notice|aria-label="Invocation output"/);
    });

    await t.test("a long log mounts only the first window of lines", () => {
      const lines = Array.from({ length: 5000 }, (_, i) => `line ${i}`);
      lines[4500] = "offscreen-widest-line".padEnd(200, "x");
      const html = output({ status: "ok", data: lines.join("\n") });
      assert.match(html, /line 32</);
      assert.doesNotMatch(html, /line 33|offscreen-widest-line/);
      assert.match(html, /width:\s*200ch/);
      // 240px viewport / 15px rows = 16 rows + 16 overscan + 1 = 33 mounted lines.
      assert.equal(html.match(/class="log-line"/g)?.length, 33);
      // Spacers carry the unmounted rows: 0 above, (5000 - 33) * 15px below.
      assert.match(html, /height:\s*0px/);
      assert.match(html, /height:\s*74505px/);
    });

    await t.test("an unreadable log shows its error without inventing log content", () => {
      const html = output({ status: "read_error", note: "permission denied" });
      assert.match(html, /read error — permission denied/);
      assert.match(html, /class="notice notice--error" role="alert"/);
      assert.doesNotMatch(html, /aria-label="Invocation output"|Loading invocation output/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
