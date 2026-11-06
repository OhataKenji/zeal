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
      return Array.from(html.matchAll(/<li\b[^>]*>([\s\S]*?)<\/li>/g), ([, content]) =>
        content
          .replace(/<[^>]*>/g, " ")
          .replace(/\s+/g, " ")
          .trim(),
      );
    }

    await t.test("same-label results keep their full identity in each row", () => {
      const html = output([
        { label: "//pkg:lib", ok: true, configuration: "cfg-a" },
        { label: "//pkg:lib", ok: true, aspect: "rustfmt", configuration: "cfg-a" },
        { label: "//pkg:lib", ok: false, aspect: "clippy", configuration: "cfg-a" },
        { label: "//pkg:lib", ok: true, configuration: "cfg-b" },
      ]);
      assert.deepEqual(rows(html), [
        "✓ //pkg:lib Target Configuration: cfg-a Completed",
        "✓ //pkg:lib Aspect: rustfmt Configuration: cfg-a Completed",
        "✗ //pkg:lib Aspect: clippy Configuration: cfg-a Failed",
        "✓ //pkg:lib Target Configuration: cfg-b Completed",
      ]);
    });

    await t.test("missing aspect and configuration render no placeholder metadata", () => {
      const html = output([
        { label: "//pkg:bare", ok: true },
        { label: "//pkg:aspect-only", ok: false, aspect: "ObjcProtoAspect" },
      ]);
      assert.deepEqual(rows(html), [
        "✓ //pkg:bare Target Completed",
        "✗ //pkg:aspect-only Aspect: ObjcProtoAspect Failed",
      ]);
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
