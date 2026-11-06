import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { Bep, Section } from "../src/lib/types.ts";

test("TargetsPanel distinguishes same-label results by aspect and configuration", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-targets-panel-"));
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
    const { default: TargetsPanel } = await server.ssrLoadModule(
      "/src/lib/invocation/TargetsPanel.svelte",
    );
    const { render } = await server.ssrLoadModule("svelte/server");

    function output(targets: Bep["targets"]): string {
      const section: Section<Bep> = {
        status: "ok",
        data: { targets, tests: [], truncated: false },
      };
      return render(TargetsPanel, { props: { section } }).body.replace(/<!--[\s\S]*?-->/g, "");
    }

    function rows(html: string): string[] {
      return html.split("<li>").slice(1);
    }

    await t.test("same-label results keep their full identity in each row", () => {
      const html = output([
        { label: "//pkg:lib", ok: true, configuration: "cfg-a" },
        { label: "//pkg:lib", ok: true, aspect: "rustfmt", configuration: "cfg-a" },
        { label: "//pkg:lib", ok: false, aspect: "clippy", configuration: "cfg-a" },
        { label: "//pkg:lib", ok: true, configuration: "cfg-b" },
      ]);
      const items = rows(html);
      assert.equal(items.length, 4);
      for (const item of items) {
        assert.match(item, /<code>\/\/pkg:lib<\/code>/);
      }
      assert.match(items[0], />\s*Target\s*<\/div>/);
      assert.doesNotMatch(items[0], /Aspect:/);
      assert.match(items[0], /Configuration:[^<]*<code>cfg-a<\/code>/);
      assert.match(items[1], /Aspect:[^<]*<code>rustfmt<\/code>/);
      assert.match(items[1], /Configuration:[^<]*<code>cfg-a<\/code>/);
      assert.match(items[2], /Aspect:[^<]*<code>clippy<\/code>/);
      assert.match(items[2], /✗/);
      assert.match(items[2], /Failed/);
      assert.match(items[3], />\s*Target\s*<\/div>/);
      assert.match(items[3], /Configuration:[^<]*<code>cfg-b<\/code>/);
      assert.equal(html.match(/Aspect:/g)?.length, 2);
      assert.equal(html.match(/Configuration:/g)?.length, 4);
      assert.equal(html.match(/cfg-a/g)?.length, 3);
    });

    await t.test("missing aspect and configuration render no placeholder metadata", () => {
      const html = output([
        { label: "//pkg:bare", ok: true },
        { label: "//pkg:aspect-only", ok: false, aspect: "ObjcProtoAspect" },
      ]);
      const items = rows(html);
      assert.equal(items.length, 2);
      assert.match(items[0], />\s*Target\s*<\/div>/);
      assert.doesNotMatch(items[0], /Aspect:|Configuration:/);
      assert.match(items[1], /Aspect:[^<]*<code>ObjcProtoAspect<\/code>/);
      assert.doesNotMatch(items[1], /Configuration:/);
    });

    await t.test("aspect values are rendered escaped", () => {
      const html = output([{ label: "//pkg:x", ok: true, aspect: "<script>alert(1)</script>" }]);
      assert.doesNotMatch(html, /<script>alert/);
      assert.match(html, /&lt;script>alert\(1\)&lt;\/script>/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
