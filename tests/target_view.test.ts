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
const HELPER: Target = {
  label: "//app/lib:helper",
  pkg: "app/lib",
  name: "helper",
  ruleClass: "java_library",
  location: null,
};
const INDEX: TargetIndex = {
  meta: { fetchedAt: 1, status: "ok" },
  targets: [PARSER, HELPER],
};
const DETAIL: TargetDetail = { target: PARSER, edges: [], files: [], status: "ok" };
const OUTPUTS: TargetOutputs = { paths: [], truncated: false };

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

    function actionRows(html: string): string[][] {
      const body = html.match(/<tbody>([\s\S]*?)<\/tbody>/)?.[1] ?? "";
      return Array.from(body.matchAll(/<tr>([\s\S]*?)<\/tr>/g), ([, row]) =>
        Array.from(row.matchAll(/<td[^>]*>([\s\S]*?)<\/td>/g), ([, cell]) =>
          cell.replace(/<[^>]*>/g, "").trim(),
        ),
      );
    }

    await t.test("columns, full output paths, and package-relative owners", () => {
      const html = view({
        actions: [
          {
            output: "bazel-out/k8-fastbuild/bin/app/lib/libparser.jar",
            mnemonic: "Javac",
            owner: "//app/lib:parser",
          },
          {
            output: "bazel-out/k8-fastbuild/bin/app/lib/libparser-src.jar",
            mnemonic: "SourceJar",
            owner: "//tools:gen",
          },
        ],
        truncated: false,
      });
      assert.match(html, /<th[^>]*>Action<\/th>/);
      assert.match(html, /<th[^>]*>Mnemonic<\/th>/);
      assert.match(html, /<th[^>]*>Owner<\/th>/);
      assert.deepEqual(actionRows(html), [
        ["bazel-out/k8-fastbuild/bin/app/lib/libparser.jar", "Javac", ":parser"],
        ["bazel-out/k8-fastbuild/bin/app/lib/libparser-src.jar", "SourceJar", "//tools:gen"],
      ]);
      assert.ok(html.includes('title="bazel-out/k8-fastbuild/bin/app/lib/libparser.jar"'));
      assert.ok(html.includes('title="//tools:gen"'));
    });

    await t.test("loading, empty, and error states", () => {
      assert.match(view(null), /Loading actions…/);
      assert.match(view({ actions: [], truncated: false }), />None</);
      assert.match(view(null, "aquery failed"), /Could not load actions: aquery failed/);
    });

    await t.test("ten rows plus a marker when truncated, none at exactly ten", () => {
      const rows = Array.from({ length: 10 }, (_, i) => ({
        output: `bazel-out/k8-fastbuild/bin/app/lib/out${i}`,
        mnemonic: "Javac",
        owner: PARSER.label,
      }));
      const truncated = view({ actions: rows, truncated: true });
      assert.equal(actionRows(truncated).length, 10);
      assert.match(truncated, /More actions not shown/);
      assert.ok(truncated.includes(">10+</span>"));

      const exact = view({ actions: rows, truncated: false });
      assert.equal(actionRows(exact).length, 10);
      assert.doesNotMatch(exact, /More actions not shown/);
      assert.ok(exact.includes(">10</span>"));
    });

    await t.test("output paths render escaped", () => {
      const html = view({
        actions: [
          {
            output: "bazel-out/<script>alert(1)</script>",
            mnemonic: "Javac",
            owner: PARSER.label,
          },
        ],
        truncated: false,
      });
      assert.doesNotMatch(html, /<script>alert/);
      assert.match(html, /&lt;script>alert\(1\)&lt;\/script>/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
