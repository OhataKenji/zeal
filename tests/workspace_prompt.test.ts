import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { TargetScope } from "../src/lib/types.ts";

const SCOPE: TargetScope = { include: ["//..."] };

test("WorkspacePrompt picks a workspace folder instead of typing a path", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-workspace-prompt-"));
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
    const { default: WorkspacePrompt } = await server.ssrLoadModule(
      "/src/lib/WorkspacePrompt.svelte",
    );
    const { render } = await server.ssrLoadModule("svelte/server");

    function output(initial: string): string {
      return render(WorkspacePrompt, {
        props: { initial, initialScope: SCOPE },
      }).body;
    }

    function tag(html: string, pattern: RegExp): string {
      const match = html.match(pattern);
      assert.ok(match, `missing element matching ${pattern}`);
      return match[0];
    }

    await t.test("an empty workspace shows a picker without a path input and disables open", () => {
      const html = output("");
      assert.doesNotMatch(html, /<input\b/);
      assert.match(html, /No folder selected/);
      const choose = tag(html, /<button[^>]*>[\s\S]*?Choose folder…[\s\S]*?<\/button>/);
      assert.match(choose, /type="button"/);
      const open = tag(html, /<button[^>]*type="submit"[^>]*>/);
      assert.match(open, /disabled/);
      assert.match(html, /Do not open an untrusted repository\./);
      assert.match(html, /target patterns/);
      assert.match(html, /scope-patterns/);
    });

    await t.test("an initial workspace is displayed as text and enables open", () => {
      const html = output("/workspace");
      assert.doesNotMatch(html, /<input\b/);
      assert.match(html, /class="workspace-path"[^>]*>\/workspace<\/span>/);
      const open = tag(html, /<button[^>]*type="submit"[^>]*>/);
      assert.doesNotMatch(open, /disabled/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
