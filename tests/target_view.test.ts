import { test } from "node:test";
import assert from "node:assert/strict";
import type {
  ConfiguredTarget,
  Target,
  TargetAction,
  TargetActions,
  TargetDetail,
  TargetIndex,
  TargetTab,
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
const HELPER: Target = {
  label: "//app/lib:helper",
  pkg: "app/lib",
  name: "helper",
  ruleClass: "cc_library",
  location: null,
};
const INDEX: TargetIndex = {
  meta: { fetchedAt: 1, status: "ok" },
  targets: [PARSER, HELPER],
};
const DETAIL: TargetDetail = { target: PARSER, edges: [], files: [], status: "ok" };
const CONFIGURED: ConfiguredTarget = {
  configuration: { mnemonic: "k8-fastbuild", checksum: "0123456789abcdef0123" },
  outputs: {
    paths: [
      "bazel-out/k8-fastbuild/bin/app/lib/libparser.jar",
      "bazel-out/k8-fastbuild/bin/app/lib/libparser-src.jar",
    ],
    truncated: false,
  },
  deps: [
    { label: "//app/lib:helper", transition: null },
    { label: "//tools:protoc", transition: "exec" },
    { label: "@bazel_tools//tools/cpp:toolchain", transition: null },
  ],
  depsTruncated: false,
};

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

  interface ViewOptions {
    actions?: TargetActions | null;
    actionsError?: string | null;
    configured?: ConfiguredTarget | null;
    configuredError?: string | null;
    tab?: TargetTab;
    flags?: string;
    target?: Target;
    index?: TargetIndex;
    detail?: TargetDetail;
  }

  function view(options: ViewOptions = {}): string {
    return render(TargetView, {
      props: {
        index: options.index ?? INDEX,
        detail: options.detail ?? { ...DETAIL, target: options.target ?? PARSER },
        configured: options.configured ?? null,
        configuredError: options.configuredError ?? null,
        actions: options.actions ?? null,
        actionsError: options.actionsError ?? null,
        tab: options.tab ?? "definition",
        flags: options.flags ?? "",
        wsRoot: "/ws",
        onselect: () => {},
        ontab: () => {},
        onapply: () => {},
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

  await t.test("three tabs render; the tab prop selects the panel", () => {
    const html = view();
    for (const label of ["Definition", "Configured", "Actions"]) {
      assert.match(html, new RegExp(`role="tab"[^>]*>${label}`));
    }
    assert.match(html, /id="target-tab-definition"[^>]*aria-selected="true"/);
    assert.match(html, /id="target-tab-configured"[^>]*aria-selected="false"/);
    assert.match(html, /id="target-tab-actions"[^>]*aria-selected="false"/);
    assert.match(html, />Attributes</);
    assert.doesNotMatch(html, /class="action-summary"/);
    assert.doesNotMatch(html, /configured-section/);

    const configured = view({ tab: "configured" });
    assert.match(configured, /id="target-tab-configured"[^>]*aria-selected="true"/);
    assert.match(configured, /Loading configuration…/);

    const actions = view({ tab: "actions", actions: { actions: [], truncated: false } });
    assert.match(actions, /id="target-tab-actions"[^>]*aria-selected="true"/);
    assert.match(actions, />None</);
    assert.doesNotMatch(actions, />Attributes</);
  });

  await t.test("flag indicators mark the expensive tabs only when flags apply", () => {
    const plain = view({ tab: "configured" });
    assert.equal((plain.match(/tab-flag/g) ?? []).length, 0);
    const flagged = view({ tab: "configured", flags: "--config=opt" });
    assert.equal((flagged.match(/tab-flag/g) ?? []).length, 2);
  });

  await t.test("definition keeps attributes and declared outs without phase reads", () => {
    const html = view();
    assert.match(html, />Attributes</);
    assert.match(html, />outs</);
    assert.doesNotMatch(html, /flag-bar/);
    assert.doesNotMatch(html, /configured-section/);

    const withOuts = view({
      detail: {
        ...DETAIL,
        files: [{ path: "app/lib/libparser.jar", owner: PARSER.label, role: "out" }],
      },
    });
    assert.ok(withOuts.includes("libparser.jar"));
  });

  await t.test("the shared flags bar shows Default or the applied flags", () => {
    const plain = view({ tab: "actions" });
    assert.match(plain, /Config:/);
    assert.match(plain, />Default</);
    assert.match(plain, /\+ Flags/);
    assert.doesNotMatch(plain, /flag-warning/);

    const custom = view({ tab: "actions", flags: "--config=opt --cpu=k8" });
    assert.ok(custom.includes("--config=opt --cpu=k8"));
    assert.match(custom, /title="--config=opt --cpu=k8"/);
    assert.match(custom, />Edit</);
    assert.match(custom, /aria-label="Reset flags"/);
    // The collapsed bar never materializes the editor or its warning.
    assert.doesNotMatch(custom, /flag-bar-edit/);
    assert.doesNotMatch(custom, /flag-warning/);
  });

  await t.test("configured shows mnemonic, hash, outputs, and deps", () => {
    const html = view({ tab: "configured", configured: CONFIGURED });
    assert.match(html, /k8-fastbuild/);
    assert.ok(html.includes("0123456789ab"));
    assert.match(html, /title="Copy configuration hash 0123456789abcdef0123"/);
    assert.match(html, /Resolved Outputs \(2\)/);
    assert.match(html, /Direct deps &amp; transitions \(3\)/);
    // Outputs display the short path; the full path stays the title.
    assert.ok(html.includes('title="bazel-out/k8-fastbuild/bin/app/lib/libparser.jar"'));
    assert.ok(html.includes(">app/lib/libparser.jar</span>"));
    assert.equal((html.match(/configured-copy/g) ?? []).length, 2);
    // Indexed deps navigate; external deps stay readable text.
    assert.match(
      html,
      /<button[^>]*class="link configured-dep mono"[^>]*>\/\/app\/lib:helper<\/button>/,
    );
    assert.match(
      html,
      /class="configured-dep configured-dep-static mono"[^>]*>@bazel_tools\/\/tools\/cpp:toolchain<\/span>/,
    );
    assert.match(html, /<span class="badge dep-transition" title="exec">exec<\/span>/);
  });

  await t.test("configured loading, error, and empty states", () => {
    assert.match(view({ tab: "configured" }), /Loading configuration…/);
    assert.match(
      view({ tab: "configured", configuredError: "cquery failed" }),
      /Could not load configuration: cquery failed/,
    );
    const empty = view({
      tab: "configured",
      configured: {
        configuration: null,
        outputs: { paths: [], truncated: false },
        deps: [],
        depsTruncated: false,
      },
    });
    assert.match(empty, /No configuration/);
    assert.match(empty, /Resolved Outputs \(0\)/);
    assert.match(empty, />None</);
  });

  await t.test("action summaries pair mnemonic with the short output path", () => {
    const html = view({
      tab: "actions",
      actions: {
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
    });
    assert.deepEqual(actionSummaries(html), [
      ["Javac", "app/lib/libparser.jar"],
      ["SourceJar", "app/lib/libparser-src.jar"],
    ]);
    // The full path stays available as the title.
    assert.ok(html.includes('title="bazel-out/k8-fastbuild/bin/app/lib/libparser.jar"'));
    // Closed rows do not materialize command details or input paths.
    assert.equal((html.match(/<details\b/g) ?? []).length, 2);
    assert.doesNotMatch(html, /class="action-details"/);
    assert.doesNotMatch(html, /<details[^>]*\bopen\b/);
  });

  await t.test("exec badge and extra-output count survive long paths", () => {
    const long = `bazel-out/k8-fastbuild/bin/app/lib/${"x".repeat(80)}.jar`;
    const html = view({
      tab: "actions",
      actions: {
        actions: [
          makeAction({
            output: long,
            outputs: [long, `${long}.src`, `${long}.tests`],
            configuration: {
              mnemonic: "Javac",
              platformName: "@local_config_platform//:host",
              checksum: "abc123",
              isTool: true,
            },
          }),
        ],
        truncated: false,
      },
    });
    assert.match(html, /<span class="badge action-exec">exec<\/span>/);
    assert.match(html, /\+2<\/span>/);
    assert.match(html, /2 additional outputs/);
  });

  await t.test("actions loading, empty, and error states", () => {
    assert.match(view({ tab: "actions" }), /Loading actions…/);
    assert.match(view({ tab: "actions", actions: { actions: [], truncated: false } }), />None</);
    assert.match(
      view({ tab: "actions", actionsError: "aquery failed" }),
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
      const truncated = view({
        tab: "actions",
        actions: { actions: rows, truncated: true },
      });
      assert.equal(actionSummaries(truncated).length, 100);
      assert.match(truncated, /Showing first 100 actions; more actions not shown\./);
      // The loaded count shows on the tab itself.
      assert.ok(truncated.includes("Actions"));
      assert.match(truncated, /\(100\+\)<\/span>/);

      const exact = view({
        tab: "actions",
        actions: { actions: rows, truncated: false },
      });
      assert.equal(actionSummaries(exact).length, 100);
      assert.doesNotMatch(exact, /Showing first 100 actions/);
      assert.match(exact, /\(100\)<\/span>/);
    },
  );

  await t.test("header shows the label, rule class, and relative source location", () => {
    const html = view({
      target: {
        ...PARSER,
        location: { file: "/ws/app/lib/BUILD.bazel", line: 14 },
      },
    });
    assert.match(html, /<h1>\/\/app\/lib:parser<\/h1>/);
    assert.ok(html.includes(PARSER.ruleClass));
    assert.match(html, />\s*app\/lib\/BUILD\.bazel:14\s*</);

    assert.ok(!view().includes("BUILD.bazel"));

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
    const html = view({ actions: { actions: [], truncated: false } });
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
        configured: null,
        configuredError: null,
        actions: { actions: [], truncated: false },
        actionsError: null,
        tab: "definition",
        flags: "",
        wsRoot: "/ws",
        onselect: () => {},
        ontab: () => {},
        onapply: () => {},
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
