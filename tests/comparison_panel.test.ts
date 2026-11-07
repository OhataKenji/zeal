import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { Bep, InvocationDetail, InvocationRecord, Section } from "../src/lib/types.ts";

function detail(
  record: Partial<InvocationRecord> = {},
  bep: Section<Bep> = { status: "ok", data: { targets: [], tests: [], truncated: false } },
): InvocationDetail {
  return {
    record: { id: "left-id", ...record },
    log: { status: "missing" },
    bep,
  };
}

test("ComparisonPanel renders the comparison form for the selected invocation", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-comparison-panel-"));
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
    const { default: ComparisonPanel } = await server.ssrLoadModule(
      "/src/lib/invocation/ComparisonPanel.svelte",
    );
    const { render } = await server.ssrLoadModule("svelte/server");

    function output(overrides: Record<string, unknown> = {}): string {
      return render(ComparisonPanel, {
        props: {
          left: detail(),
          leftLoading: false,
          leftError: null,
          activeInvocation: null,
          ...overrides,
        },
      }).body.replace(/<!--[\s\S]*?-->/g, "");
    }

    function inputTag(html: string): string {
      const input = html.match(/<input[^>]*id="compare-invocation-id"[^>]*>/)?.[0];
      assert.ok(input, "comparison input is missing");
      return input;
    }

    await t.test("shows the selected ID with a labeled, enabled input", () => {
      const html = output();
      assert.ok(html.includes("left-id"));
      assert.match(
        html,
        /<label[^>]*for="compare-invocation-id"[^>]*>\s*Comparison invocation\s*<\/label>/,
      );
      assert.doesNotMatch(inputTag(html), /\bdisabled\b/);
      assert.match(inputTag(html), /placeholder="[^"]+"/);
    });

    await t.test("disables the input while the left detail is loading", () => {
      const html = output({ left: null, leftLoading: true });
      assert.match(inputTag(html), /\bdisabled\b/);
      assert.match(html, /Loading…/);
    });

    await t.test("disables the input when the left detail failed", () => {
      const html = output({ leftError: "read failed" });
      assert.match(inputTag(html), /\bdisabled\b/);
      assert.ok(html.includes("read failed"));
    });

    await t.test("disables the input while the compared invocation is active", () => {
      const html = output({ activeInvocation: "left-id" });
      assert.match(inputTag(html), /\bdisabled\b/);
      assert.ok(html.includes("Comparison is available"));
    });

    await t.test("keeps the input enabled when a different invocation is active", () => {
      const html = output({ activeInvocation: "other-id" });
      assert.doesNotMatch(inputTag(html), /\bdisabled\b/);
    });

    await t.test("escapes option values in the rendered HTML", () => {
      const html = output({
        left: detail(
          {},
          {
            status: "ok",
            data: {
              targets: [],
              tests: [],
              truncated: false,
              options: {
                startupOptions: [],
                explicitStartupOptions: [],
                cmdLine: ["<script>alert(1)</script>"],
                explicitCmdLine: [],
              },
            },
          },
        ),
      });
      assert.doesNotMatch(html, /<script>alert/);
      assert.match(html, /&lt;script>alert\(1\)&lt;\/script>/);
    });

    await t.test("surfaces a non-ok BEP section as a notice", () => {
      const html = output({
        left: detail(
          {},
          {
            status: "incomplete",
            data: { targets: [], tests: [], truncated: false },
            note: "saved prefix missing",
          },
        ),
      });
      assert.ok(html.includes("saved prefix missing"));
    });

    await t.test("InvocationView shows the Compare tab without the form on All", async () => {
      const { emptyView } = await server!.ssrLoadModule("/src/lib/session.ts");
      const { default: InvocationView } = await server!.ssrLoadModule(
        "/src/lib/InvocationView.svelte",
      );
      const left = detail();
      const view = {
        ...emptyView(),
        selectedInvocation: "left-id",
        invocations: [left.record],
        detail: left,
      };
      const html = render(InvocationView, {
        props: { view, reload: () => {}, reloadProfile: () => {} },
      }).body.replace(/<!--[\s\S]*?-->/g, "");
      assert.match(html, /id="invocation-tab-Compare"/);
      assert.ok(!html.includes("compare-invocation-id"));
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
