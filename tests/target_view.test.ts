import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type {
  Target,
  TargetAction,
  TargetActions,
  TargetDetail,
  TargetIndex,
  TargetOutputs,
} from "../src/lib/types.ts";

const PARSER: Target = {
  label: "//app/lib:parser",
  pkg: "app/lib",
  name: "parser",
  ruleClass: "java_library",
  location: null,
};
const INDEX: TargetIndex = {
  meta: { fetchedAt: 1, status: "ok" },
  targets: [PARSER],
};
const DETAIL: TargetDetail = { target: PARSER, edges: [], files: [], status: "ok" };
const OUTPUTS: TargetOutputs = { paths: [], truncated: false };

function makeAction(overrides: Partial<TargetAction> = {}): TargetAction {
  const output = overrides.output ?? "bazel-out/k8-fastbuild/bin/app/lib/libparser.jar";
  return {
    output,
    mnemonic: "Javac",
    owner: PARSER.label,
    arguments: [],
    inputs: [],
    outputs: [output],
    environmentVariables: [],
    configuration: null,
    executionPlatform: "",
    ...overrides,
  };
}

test("TargetView renders the actions table and its states", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-target-view-"));
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
    const { default: TargetView } = await server.ssrLoadModule("/src/lib/TargetView.svelte");
    const { render } = await server.ssrLoadModule("svelte/server");

    function view(actions: TargetActions | null, actionsError: string | null = null): string {
      return render(TargetView, {
        props: {
          index: INDEX,
          detail: DETAIL,
          outputs: OUTPUTS,
          outputsError: null,
          actions,
          actionsError,
          wsRoot: "/ws",
          onselect: () => {},
        },
      }).body.replace(/<!--[\s\S]*?-->/g, "");
    }

    function actionSummaries(html: string): string[][] {
      return Array.from(
        html.matchAll(/<summary class="action-summary">([\s\S]*?)<\/summary>/g),
        ([, summary]) =>
          Array.from(
            summary.matchAll(/<span class="action-(?:mnemonic|output)"[^>]*>([\s\S]*?)<\/span>/g),
            ([, text]) => text.replace(/<[^>]*>/g, "").trim(),
          ),
      );
    }

    await t.test("mnemonic and primary output columns with collapsed detail rows", () => {
      const html = view({
        actions: [
          makeAction({
            output: "bazel-out/k8-fastbuild/bin/app/lib/libparser.jar",
            mnemonic: "Javac",
          }),
          makeAction({
            output: "bazel-out/k8-fastbuild/bin/app/lib/libparser-src.jar",
            mnemonic: "SourceJar",
            owner: "//tools:gen",
          }),
        ],
        truncated: false,
      });
      assert.match(html, /<th[^>]*>Mnemonic<\/th>/);
      assert.match(html, /<th[^>]*>Primary output<\/th>/);
      assert.doesNotMatch(html, /<th[^>]*>(Action|Owner)<\/th>/);
      assert.deepEqual(actionSummaries(html), [
        ["Javac", "bazel-out/k8-fastbuild/bin/app/lib/libparser.jar"],
        ["SourceJar", "bazel-out/k8-fastbuild/bin/app/lib/libparser-src.jar"],
      ]);
      // Rows start collapsed: details render their content but carry no open
      // attribute, so the browser keeps every section hidden.
      assert.equal((html.match(/<details>/g) ?? []).length, 2);
      assert.equal((html.match(/class="action-details"/g) ?? []).length, 2);
      assert.doesNotMatch(html, /<details[^>]*\bopen\b/);
    });

    await t.test("extra outputs are counted next to the primary output", () => {
      const html = view({
        actions: [
          makeAction({
            outputs: [
              "bazel-out/k8-fastbuild/bin/app/lib/libparser.jar",
              "bazel-out/k8-fastbuild/bin/app/lib/libparser-src.jar",
              "bazel-out/k8-fastbuild/bin/app/lib/libparser-tests.jar",
            ],
          }),
        ],
        truncated: false,
      });
      assert.match(html, /\+2<\/span>/);
      assert.match(html, /2 additional outputs/);
    });

    await t.test("loading, empty, and error states", () => {
      assert.match(view(null), /Loading actions…/);
      assert.match(view({ actions: [], truncated: false }), />None</);
      assert.match(view(null, "aquery failed"), /Could not load actions: aquery failed/);
    });

    await t.test(
      "one hundred rows plus a message when truncated, none at exactly one hundred",
      () => {
        const rows = Array.from({ length: 100 }, (_, i) =>
          makeAction({
            output: `bazel-out/k8-fastbuild/bin/app/lib/out${i}`,
            outputs: [`bazel-out/k8-fastbuild/bin/app/lib/out${i}`],
          }),
        );
        const truncated = view({ actions: rows, truncated: true });
        assert.equal(actionSummaries(truncated).length, 100);
        assert.match(truncated, /Showing first 100 actions; more actions not shown\./);
        assert.ok(truncated.includes(">100+</span>"));

        const exact = view({ actions: rows, truncated: false });
        assert.equal(actionSummaries(exact).length, 100);
        assert.doesNotMatch(exact, /Showing first 100 actions/);
        assert.ok(exact.includes(">100</span>"));
      },
    );
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
