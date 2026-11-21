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
import { splitLabel } from "../src/lib/model.ts";
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
    inputsTruncated: false,
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
    initialTab: "definition" | "actions" = "definition",
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
        initialTab,
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
    const html = view(
      {
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
      },
      null,
      PARSER,
      "actions",
    );
    assert.match(html, /<th[^>]*>Mnemonic<\/th>/);
    assert.match(html, /<th[^>]*>Primary output<\/th>/);
    assert.doesNotMatch(html, /<th[^>]*>(Action|Owner)<\/th>/);
    assert.deepEqual(actionSummaries(html), [
      ["Javac", "bazel-out/k8-fastbuild/bin/app/lib/libparser.jar"],
      ["SourceJar", "bazel-out/k8-fastbuild/bin/app/lib/libparser-src.jar"],
    ]);
    // Closed rows do not materialize command details or input paths in the DOM.
    assert.equal((html.match(/<details\b/g) ?? []).length, 2);
    assert.doesNotMatch(html, /class="action-details"/);
    assert.doesNotMatch(html, /<details[^>]*\bopen\b/);
  });

  await t.test("extra outputs are counted next to the primary output", () => {
    const html = view(
      {
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
      },
      null,
      PARSER,
      "actions",
    );
    assert.match(html, /\+2<\/span>/);
    assert.match(html, /2 additional outputs/);
  });

  await t.test("loading, empty, and error states", () => {
    assert.match(view(null, null, PARSER, "actions"), /Loading actions…/);
    assert.match(view({ actions: [], truncated: false }, null, PARSER, "actions"), />None</);
    assert.match(
      view(null, "aquery failed", PARSER, "actions"),
      /Could not load actions: aquery failed/,
    );
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
      const truncated = view({ actions: rows, truncated: true }, null, PARSER, "actions");
      assert.equal(actionSummaries(truncated).length, 100);
      assert.match(truncated, /Showing first 100 actions; more actions not shown\./);
      assert.ok(truncated.includes(">100+</span>"));

      const exact = view({ actions: rows, truncated: false }, null, PARSER, "actions");
      assert.equal(actionSummaries(exact).length, 100);
      assert.doesNotMatch(exact, /Showing first 100 actions/);
      assert.ok(exact.includes(">100</span>"));
    },
  );

  await t.test("definition is the default tab and actions renders via initialTab", () => {
    const definition = view({ actions: [], truncated: false });
    assert.match(definition, /role="tab"[^>]*aria-selected="true"[^>]*>Definition/);
    assert.match(definition, />Outputs</);
    assert.match(definition, />Attributes</);
    assert.doesNotMatch(definition, /class="action-summary"/);

    const actions = view({ actions: [], truncated: false }, null, PARSER, "actions");
    assert.match(actions, /role="tab"[^>]*aria-selected="true"[^>]*>Actions \(0\)/);
    assert.match(actions, />None</);
    assert.doesNotMatch(actions, />Outputs</);
    assert.doesNotMatch(actions, />Attributes</);
  });

  await t.test("header shows the label, rule class, and relative source location", () => {
    const html = view({ actions: [], truncated: false }, null, {
      ...PARSER,
      location: { file: "/ws/app/lib/BUILD.bazel", line: 14 },
    });
    assert.match(html, /<h1>\/\/app\/lib:parser<\/h1>/);
    assert.ok(html.includes(PARSER.ruleClass));
    assert.match(html, />\s*app\/lib\/BUILD\.bazel:14\s*</);

    assert.ok(!view({ actions: [], truncated: false }).includes("BUILD.bazel"));

    const pkgHtml = render(PackageView, {
      props: {
        index: INDEX,
        pkg: "app/lib",
        onselect: () => {},
        onrun: () => {},
        onopenflags: () => {},
      },
    }).body.replace(/<!--[\s\S]*?-->/g, "");
    assert.match(pkgHtml, /<h1[^>]*>\/\/app\/lib:all<\/h1>/);
    assert.match(pkgHtml, />:parser</);
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

test("PackageView scope labels, root //..., and the row cap", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: PackageView } = await load("/src/lib/PackageView.svelte");

  function indexedTarget(label: string): Target {
    return { label, ...splitLabel(label), ruleClass: "cc_library", location: null };
  }

  function output(index: TargetIndex, pkg: string): string {
    return render(PackageView, {
      props: { index, pkg, onselect: () => {}, onrun: () => {}, onopenflags: () => {} },
    })
      .body.replace(/<!--[\s\S]*?-->/g, "")
      .replace(/\s+/g, " ");
  }

  const MIXED: TargetIndex = {
    meta: { fetchedAt: 1, status: "ok" },
    targets: [
      indexedTarget("//app:top"),
      indexedTarget("//app/lib:deep"),
      indexedTarget("//other:solo"),
      indexedTarget("//:root"),
    ],
  };

  await t.test('pkg "" is //... with full labels and no ///...', () => {
    const html = output(MIXED, "");
    assert.match(html, /<h1[^>]*>\/\/\.\.\.<\/h1>/);
    assert.doesNotMatch(html, /\/\/\/\.\.\.|\/\/:all/);
    // Every target in the workspace is listed under its full label.
    for (const label of ["//app:top", "//app/lib:deep", "//other:solo", "//:root"]) {
      assert.ok(html.includes(`>${label}</button>`), `missing ${label}`);
    }
    assert.doesNotMatch(html, />:top</);
  });

  await t.test("sub-packages widen the scope to //pkg/... and keep full labels", () => {
    const html = output(MIXED, "app");
    assert.match(html, /<h1[^>]*>\/\/app\/\.\.\.<\/h1>/);
    assert.ok(html.includes(">:top</button>"));
    assert.ok(html.includes(">//app/lib:deep</button>"));
    assert.ok(!html.includes(">//other:solo</button>"));
  });

  await t.test("a leaf package scopes to //pkg:all", () => {
    const html = output(MIXED, "other");
    assert.match(html, /<h1[^>]*>\/\/other:all<\/h1>/);
    assert.ok(html.includes(">:solo</button>"));
    assert.ok(!html.includes(">//other:solo</button>"));
  });

  await t.test("sorting does not mutate index.targets", () => {
    const index: TargetIndex = {
      meta: { fetchedAt: 1, status: "ok" },
      targets: [indexedTarget("//z:-last"), indexedTarget("//a:first")],
    };
    const before = index.targets.map((t) => t.label);
    output(index, "");
    assert.deepEqual(
      index.targets.map((t) => t.label),
      before,
    );
    const html = output(index, "");
    assert.ok(html.indexOf(">//a:first</button>") < html.indexOf(">//z:-last</button>"));
  });

  await t.test("more than 500 rows truncate with a notice", () => {
    const index: TargetIndex = {
      meta: { fetchedAt: 1, status: "ok" },
      targets: Array.from({ length: 501 }, (_, i) =>
        indexedTarget(`//app:t${String(i).padStart(4, "0")}`),
      ),
    };
    const html = output(index, "app");
    assert.equal((html.match(/class="clickable"/g) ?? []).length, 500);
    assert.match(
      html,
      /Showing first 500 of 501 targets — select a package or press ⌘K to narrow\./,
    );

    const exact: TargetIndex = {
      meta: { fetchedAt: 1, status: "ok" },
      targets: index.targets.slice(0, 500),
    };
    const exactHtml = output(exact, "app");
    assert.equal((exactHtml.match(/class="clickable"/g) ?? []).length, 500);
    assert.doesNotMatch(exactHtml, /Showing first 500/);
  });
});
