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

    function output(initial: string, include = SCOPE.include): string {
      return render(WorkspacePrompt, {
        props: { initial, initialScope: { include } },
      })
        .body.replace(/<!--[\s\S]*?-->/g, "")
        .replace(/\s+/g, " ");
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

    await t.test("the default workspace-wide scope shows an advisory warning", () => {
      const html = output("/workspace");
      assert.match(html, /id="scope-warning"/);
      assert.match(html, /may be slow in large workspaces/);
      assert.match(html, /Consider using a narrower scope/);
      assert.match(tag(html, /<textarea\b[^>]*>/), /aria-describedby="scope-help scope-warning"/);
      assert.doesNotMatch(tag(html, /<button[^>]*type="submit"[^>]*>/), /disabled/);
    });

    for (const include of [[" \t//... \t"], ["//app/...", "", " //... ", "//tools:all"]]) {
      await t.test(
        `workspace-wide patterns warn after trimming: ${JSON.stringify(include)}`,
        () => {
          const html = output("/workspace", include);
          assert.match(html, /id="scope-warning"/);
          assert.match(
            tag(html, /<textarea\b[^>]*>/),
            /aria-describedby="scope-help scope-warning"/,
          );
          assert.doesNotMatch(tag(html, /<button[^>]*type="submit"[^>]*>/), /disabled/);
        },
      );
    }

    for (const include of [["//app/..."], ["//app:all"], ["", " \t", ""]]) {
      await t.test(`narrow or blank patterns do not warn: ${JSON.stringify(include)}`, () => {
        const html = output("/workspace", include);
        assert.doesNotMatch(html, /scope-warning/);
        assert.match(tag(html, /<textarea\b[^>]*>/), /aria-describedby="scope-help"/);
      });
    }
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
