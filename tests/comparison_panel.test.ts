import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { Bep, InvocationDetail, InvocationRecord, Section } from "../src/lib/types.ts";

const OPTS: NonNullable<Bep["options"]> = {
  startupOptions: ["--startup"],
  explicitStartupOptions: ["--explicit-startup"],
  cmdLine: ["--cmd", "value with space"],
  explicitCmdLine: ["--explicit-cmd"],
};

const LABELS = [
  "Invocation ID",
  "Command",
  "Pattern",
  "Exit code",
  "Start date",
  "Duration",
  "Explicit command line",
  "Full command line",
  "Explicit startup options",
  "Full startup options",
];

function detail(
  record: Partial<InvocationRecord> = {},
  bep: Section<Bep> = {
    status: "ok",
    data: { targets: [], tests: [], truncated: false, options: OPTS },
  },
): InvocationDetail {
  return {
    record: {
      id: "left-id",
      cmd: "build //app:all",
      pattern: "//app:all",
      exitCode: 0,
      startedAt: 1700000000000,
      finishedAt: 1700000001000,
      ...record,
    },
    log: { status: "missing" },
    bep,
  };
}

test("ComparisonPanel renders rows, placeholders, and the ID form", async (t) => {
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

    function output(props: Record<string, unknown>): string {
      return render(ComparisonPanel, { props }).body.replace(/<!--[\s\S]*?-->/g, "");
    }

    function panelProps(overrides: Record<string, unknown> = {}): Record<string, unknown> {
      return {
        left: detail(),
        leftLoading: false,
        leftError: null,
        activeInvocation: null,
        ...overrides,
      };
    }

    function inputTag(html: string): string {
      return html.match(/<input[^>]*id="compare-invocation-id"[^>]*>/)?.[0] ?? "";
    }

    function rowLabels(html: string): string[] {
      return Array.from(
        html.matchAll(/<th[^>]*scope="row"[^>]*>([\s\S]*?)<\/th>/g),
        ([, content]) =>
          content
            .replace(/<[^>]*>/g, " ")
            .replace(/\s+/g, " ")
            .trim(),
      );
    }

    await t.test("renders ten labeled rows with left values and right placeholders", () => {
      const html = output(panelProps());
      assert.deepEqual(rowLabels(html), LABELS);
      assert.equal(html.match(/<code>—<\/code>/g)?.length, 10);
      assert.ok(!html.includes("Not recorded"));
      assert.ok(html.includes("build //app:all"));
      assert.ok(html.includes("left-id"));
      assert.ok(html.includes("1.000s"));
    });

    await t.test("renders a labeled ID form with an enabled input when ready", () => {
      const html = output(panelProps());
      assert.match(
        html,
        /<label[^>]*for="compare-invocation-id"[^>]*>\s*Comparison invocation\s*<\/label>/,
      );
      assert.ok(inputTag(html).includes('placeholder="Enter an invocation ID and press Enter."'));
      assert.ok(!html.includes("compare-hint"));
      assert.ok(!html.includes(">Item<"));
      assert.equal(html.match(/scope="row"/g)?.length, 10);
      assert.doesNotMatch(inputTag(html), /\bdisabled\b/);
    });

    await t.test("disables the input while left is loading, errored, or active", () => {
      const loading = output(panelProps({ left: null, leftLoading: true }));
      assert.match(inputTag(loading), /\bdisabled\b/);
      assert.match(loading, /Loading…/);

      const errored = output(panelProps({ leftError: "read failed" }));
      assert.match(inputTag(errored), /\bdisabled\b/);
      assert.ok(errored.includes("read failed"));

      const active = output(panelProps({ activeInvocation: "left-id" }));
      assert.match(inputTag(active), /\bdisabled\b/);
      assert.ok(active.includes("Comparison is available after the invocation finishes."));

      const otherActive = output(panelProps({ activeInvocation: "other-id" }));
      assert.doesNotMatch(inputTag(otherActive), /\bdisabled\b/);
    });

    await t.test("escapes option values in the rendered HTML", () => {
      const html = output(
        panelProps({
          left: detail(
            {},
            {
              status: "ok",
              data: {
                targets: [],
                tests: [],
                truncated: false,
                options: { ...OPTS, cmdLine: ["<script>alert(1)</script>"] },
              },
            },
          ),
        }),
      );
      assert.doesNotMatch(html, /<script>alert/);
      assert.match(html, /&lt;script>alert\(1\)&lt;\/script>/);
    });

    await t.test("surfaces a non-ok BEP section as a notice", () => {
      const html = output(panelProps({ left: detail({}, { status: "missing" }) }));
      assert.ok(html.includes("missing — no record"));
    });

    await t.test(
      "InvocationView exposes a Compare tab after Timing without the form on All",
      async () => {
        const { emptyView } = await server!.ssrLoadModule("/src/lib/session.ts");
        const { default: InvocationView } = await server!.ssrLoadModule(
          "/src/lib/InvocationView.svelte",
        );
        const view = {
          ...emptyView(),
          selectedInvocation: "left-id",
          invocations: [detail().record],
          detail: detail(),
        };
        const html = render(InvocationView, {
          props: { view, reload: () => {}, reloadProfile: () => {} },
        }).body.replace(/<!--[\s\S]*?-->/g, "");
        const tabs = Array.from(html.matchAll(/id="invocation-tab-([^"]+)"/g), ([, tab]) => tab);
        assert.deepEqual(tabs, ["All", "Targets", "Details", "Logs", "Timing", "Compare"]);
        assert.ok(!html.includes("compare-invocation-id"));
      },
    );
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
