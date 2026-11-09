import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

test("CopyButton renders a labeled icon button", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-copy-button-"));
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
    const { default: CopyButton } = await server.ssrLoadModule("/src/lib/CopyButton.svelte");
    const { render } = await server.ssrLoadModule("svelte/server");

    function output(overrides: { disabled?: boolean } = {}): string {
      return render(CopyButton, {
        props: {
          text: "build //...",
          label: "Copy settings",
          onerror: () => {},
          ...overrides,
        },
      }).body;
    }

    function tag(html: string, pattern: RegExp): string {
      const match = html.match(pattern);
      assert.ok(match, `missing element matching ${pattern}`);
      return match[0];
    }

    await t.test("renders an enabled labeled button with an icon", () => {
      const html = output();
      const button = tag(html, /<button[^>]*>/);
      assert.match(button, /type="button"/);
      assert.match(button, /aria-label="Copy settings"/);
      assert.match(button, /title="Copy settings"/);
      assert.doesNotMatch(button, /disabled/);
      assert.match(html, /<svg/);
    });

    await t.test("renders disabled when asked", () => {
      const button = tag(output({ disabled: true }), /<button[^>]*>/);
      assert.match(button, /disabled/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
