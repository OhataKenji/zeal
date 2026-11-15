import { test } from "node:test";
import assert from "node:assert/strict";
import type {
  Target,
  TargetAction,
  TargetActions,
  TargetDetail,
  TargetIndex,
  TargetOutputs,
} from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

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

test("TargetView renders target details", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: TargetView } = await load("/src/lib/TargetView.svelte");
  const { default: PackageView } = await load("/src/lib/PackageView.svelte");

  function view(
    actions: TargetActions | null,
    actionsError: string | null = null,
    target: Target = PARSER,
  ): string {
    return render(TargetView, {
      props: {
        index: INDEX,
        detail: { ...DETAIL, target },
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

  await t.test("header keeps label, kind, and source as one wrapping row", () => {
    const GRAMMARS: Target = {
      label: "//app:grammars",
      pkg: "app",
      name: "grammars",
      ruleClass: "filegroup",
      location: { file: "/ws/app/BUILD.bazel", line: 14 },
    };
    const html = view({ actions: [], truncated: false }, null, GRAMMARS);
    const header = html.match(/<header class="target-head">([\s\S]*?)<\/header>/)?.[1] ?? "";
    const h1 = header.indexOf("<h1");
    const badge = header.indexOf("filegroup");
    const source = header.indexOf("app/BUILD.bazel:14");
    assert.ok(h1 !== -1 && badge !== -1 && source !== -1);
    assert.ok(h1 < badge && badge < source);
    assert.match(header, /title="\/ws\/app\/BUILD\.bazel"/);

    assert.ok(!view({ actions: [], truncated: false }).includes("source-location"));

    const LONG: Target = {
      label: "//tools/a-very-long-target-name-for-header-wrapping-checks",
      pkg: "tools",
      name: "a-very-long-target-name-for-header-wrapping-checks",
      ruleClass: "genrule",
      location: {
        file: "/ws/tools/deeply/nested/directory/structure/BUILD.bazel",
        line: 42,
      },
    };
    const longHtml = view({ actions: [], truncated: false }, null, LONG);
    assert.ok(longHtml.includes(LONG.label));
    assert.ok(longHtml.includes("/ws/tools/deeply/nested/directory/structure/BUILD.bazel"));
    assert.ok(longHtml.includes("tools/deeply/nested/directory/structure/BUILD.bazel:42"));

    const pkgHtml = render(PackageView, {
      props: { index: INDEX, pkg: "app/lib", onselect: () => {} },
    }).body.replace(/<!--[\s\S]*?-->/g, "");
    const pkgHeader = pkgHtml.match(/<header class="target-head">([\s\S]*?)<\/header>/)?.[1] ?? "";
    assert.ok(pkgHeader.indexOf("<h1") < pkgHeader.indexOf("Package"));
  });

  await t.test("attributes render without the label filter and keep Show more", () => {
    const html = view({ actions: [], truncated: false });
    assert.ok(!html.includes("relation-filter"));
    assert.ok(!html.includes("Filter labels"));
    assert.ok(!html.includes("No matching"));
    assert.ok((html.match(/>None</g) ?? []).length >= 4);

    const deps = Array.from({ length: 14 }, (_, i) => ({
      label: `//dep:lib${String(i).padStart(2, "0")}`,
      pkg: "dep",
      name: `lib${String(i).padStart(2, "0")}`,
      ruleClass: "cc_library",
      location: null,
    }));
    const MANY: TargetDetail = {
      target: PARSER,
      edges: Array.from({ length: 14 }, (_, i) => ({
        from: PARSER.label,
        to: `//dep:lib${String(i).padStart(2, "0")}`,
        attr: "attrdeps",
      })),
      files: Array.from({ length: 18 }, (_, i) => ({
        path: `app/lib/file${String(i).padStart(2, "0")}.ts`,
        owner: PARSER.label,
        role: "srcs",
      })),
      status: "ok",
    };
    const manyHtml = render(TargetView, {
      props: {
        index: { ...INDEX, targets: [...INDEX.targets, ...deps] },
        detail: MANY,
        outputs: OUTPUTS,
        outputsError: null,
        actions: { actions: [], truncated: false },
        actionsError: null,
        wsRoot: "/ws",
        onselect: () => {},
      },
    }).body.replace(/<!--[\s\S]*?-->/g, "");

    function ddFor(name: string): string {
      const start = manyHtml.indexOf(`<dt>${name}</dt>`);
      assert.ok(start !== -1, `missing attribute ${name}`);
      const ddStart = manyHtml.indexOf("<dd>", start);
      const ddEnd = manyHtml.indexOf("</dd>", ddStart);
      return manyHtml.slice(ddStart, ddEnd);
    }

    const depsDd = ddFor("attrdeps");
    assert.equal((depsDd.match(/class="chip"/g) ?? []).length, 12);
    assert.match(depsDd, /\+2 more/);
    assert.match(depsDd, /aria-expanded="false"/);

    const srcsDd = ddFor("srcs");
    assert.equal((srcsDd.match(/class="chip file"/g) ?? []).length, 16);
    assert.match(srcsDd, /\+2 more/);
    assert.match(srcsDd, /aria-expanded="false"/);

    assert.match(ddFor("data"), />None</);
    assert.match(ddFor("deps"), />None</);
  });
});
