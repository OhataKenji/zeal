import { test } from "node:test";
import assert from "node:assert/strict";
import {
  ancestorsOf,
  buildTree,
  commandLine,
  flatten,
  isTestRule,
  relations,
  shellQuote,
  splitLabel,
  subtreeTargets,
} from "../src/lib/model.ts";
import type { Target, TargetDetail } from "../src/lib/types.ts";

function target(label: string, ruleClass = "cc_library"): Target {
  return { label, ...splitLabel(label), ruleClass, location: null };
}

// Deliberately unsorted labels across nested packages plus a root target.
function treeFixture(): Target[] {
  return [
    target("//zeta:top"),
    target("//app:server"),
    target("//app/lib:parser"),
    target("//app:alpha"),
    target("//:root_tool"),
    target("//app/api:handler"),
  ];
}

function rowKey(row: ReturnType<typeof flatten>[number]): string {
  return row.kind === "dir"
    ? `dir:${row.node.path}@${row.depth}`
    : `tgt:${row.target.label}@${row.depth}`;
}

test("package tree sorts directories and targets and counts subtree totals", () => {
  const { root: tree, byPath } = buildTree(treeFixture());
  assert.equal(tree.count, 6);
  assert.equal(byPath.get(""), tree);
  assert.deepEqual(
    tree.dirs.map((d) => d.path),
    ["app", "zeta"],
  );
  assert.deepEqual(
    tree.targets.map((t) => t.name),
    ["root_tool"],
  );

  const app = tree.dirs[0];
  // app holds two direct targets plus one target in each child package.
  assert.equal(app.count, 4);
  assert.deepEqual(
    app.dirs.map((d) => d.path),
    ["app/api", "app/lib"],
  );
  assert.deepEqual(
    app.targets.map((t) => t.name),
    ["alpha", "server"],
  );
  assert.equal(app.dirs[0].count, 1);
  assert.equal(app.dirs[1].count, 1);
  assert.equal(tree.dirs[1].count, 1);
  // byPath hands out the same node objects the tree rows render.
  assert.equal(byPath.get("app/lib"), app.dirs[1]);
});

test("subtreeTargets puts direct targets first, then descendants by label", () => {
  const generated = Array.from(
    { length: 501 },
    (_, i) => `//app/lib:t${String(i).padStart(4, "0")}`,
  );
  const { root, byPath } = buildTree([
    target("//app:z"),
    target("//app/deeper:x"),
    target("//app2:sibling"),
    target("//:root"),
    ...generated.map((label) => target(label)),
  ]);
  const app = byPath.get("app");
  assert.ok(app);
  // The root's own target leads; descendants follow in label order.
  assert.deepEqual(
    subtreeTargets(root)
      .map((t) => t.label)
      .slice(0, 2),
    ["//:root", "//app/deeper:x"],
  );
  // //app's direct target leads its descendants; //app2 stays out.
  assert.deepEqual(
    subtreeTargets(app).map((t) => t.label),
    ["//app:z", "//app/deeper:x", ...generated],
  );
});

test("flatten lists directories before target leaves at every level", () => {
  const { root: tree } = buildTree(treeFixture());

  // Collapsed: only depth-0 rows — directories first, then the root leaves.
  assert.deepEqual(flatten(tree, new Set()).map(rowKey), [
    "dir:app@0",
    "dir:zeta@0",
    "tgt://:root_tool@0",
  ]);

  // Expanded app and app/lib: each level still leads with its directories.
  const rows = flatten(tree, new Set(["app", "app/lib"]));
  assert.deepEqual(rows.map(rowKey), [
    "dir:app@0",
    "dir:app/api@1",
    "dir:app/lib@1",
    "tgt://app/lib:parser@2",
    "tgt://app:alpha@1",
    "tgt://app:server@1",
    "dir:zeta@0",
    "tgt://:root_tool@0",
  ]);
  assert.equal(rows[0].kind === "dir" && rows[0].expanded, true);
  assert.equal(rows[1].kind === "dir" && rows[1].expanded, false);
  assert.equal(rows[2].kind === "dir" && rows[2].expanded, true);

  // There is no synthetic //... row: the root only contributes its own leaves.
  assert.ok(rows.every((row) => row.kind !== "dir" || row.node.path !== ""));
});

test("revealing a target expands its package ancestors including the package itself", () => {
  const { root: tree } = buildTree(treeFixture());
  const expanded = new Set<string>();
  for (const path of ancestorsOf(splitLabel("//app/lib:parser").pkg)) {
    expanded.add(path);
  }
  // ancestorsOf is inclusive: "app/lib" opens so its :parser leaf shows.
  assert.deepEqual([...expanded].sort(), ["app", "app/lib"]);
  const leaf = flatten(tree, expanded).find(
    (row) => row.kind === "target" && row.target.label === "//app/lib:parser",
  );
  assert.ok(leaf);
  assert.equal(leaf.depth, 2);
  assert.deepEqual(ancestorsOf("app/lib"), ["app", "app/lib"]);
  assert.deepEqual(ancestorsOf(""), []);
});

test("isTestRule covers test rules and test suites only", () => {
  assert.equal(isTestRule("cc_test"), true);
  assert.equal(isTestRule("java_test"), true);
  assert.equal(isTestRule("test_suite"), true);
  assert.equal(isTestRule("cc_library"), false);
  assert.equal(isTestRule("contest"), false);
});

test("forward attributes retain target and file inputs under their original names", () => {
  const detail: TargetDetail = {
    target: {
      label: "//app:generated",
      pkg: "app",
      name: "generated",
      ruleClass: "genrule",
      location: null,
    },
    edges: [
      { from: "//app:generated", to: "//app:inputs", attr: "srcs" },
      { from: "//app:generated", to: "//app:inputs", attr: "srcs" },
      { from: "//app:generated", to: "//tools:compiler", attr: "tools" },
    ],
    files: [
      { owner: "//app:generated", path: "app/input.txt", role: "srcs" },
      { owner: "//app:generated", path: "app/runtime.txt", role: "data" },
      { owner: "//app:generated", path: "app/helper.sh", role: "tools" },
      { owner: "//app:generated", path: "app/generated.txt", role: "out" },
      { owner: "//other:target", path: "other/input.txt", role: "srcs" },
    ],
    status: "ok",
  };
  const result = relations(detail);
  assert.deepEqual(result?.attributes, [
    { name: "srcs", labels: ["//app:inputs"], files: ["app/input.txt"] },
    { name: "data", labels: [], files: ["app/runtime.txt"] },
    { name: "deps", labels: [], files: [] },
    { name: "tools", labels: ["//tools:compiler"], files: ["app/helper.sh"] },
  ]);
  assert.deepEqual(
    result?.outputs.map((file) => file.path),
    ["app/generated.txt"],
  );
});

test("shellQuote keeps safe arguments bare and quotes the rest", () => {
  assert.equal(shellQuote("javac"), "javac");
  assert.equal(shellQuote("-classpath"), "-classpath");
  assert.equal(shellQuote("app/lib/parser.jar"), "app/lib/parser.jar");
  assert.equal(shellQuote("a=b,c:d@e%f+g"), "a=b,c:d@e%f+g");
  assert.equal(shellQuote("two words"), "'two words'");
  assert.equal(shellQuote(""), "''");
  assert.equal(shellQuote("it's"), `'it'\\''s'`);
  assert.equal(shellQuote('a;b|c&d>e<f(g)h$i`j"k\\l\nm\to'), "'a;b|c&d>e<f(g)h$i`j\"k\\l\nm\to'");
  assert.equal(shellQuote("#hash"), "'#hash'");
  assert.equal(shellQuote("~/home"), "'~/home'");
  assert.equal(shellQuote("*.{cc,h}"), "'*.{cc,h}'");
});

test("commandLine joins quoted arguments with spaces", () => {
  assert.equal(commandLine({ arguments: [] }), "");
  assert.equal(
    commandLine({ arguments: ["gcc", "-DFOO=bar baz", "it's.h", "-o", "out.o"] }),
    `'gcc' '-DFOO=bar baz' 'it'\\''s.h' -o out.o`,
  );
  // argv[0] is always quoted so reserved words and assignments cannot be
  // reinterpreted as shell syntax.
  assert.equal(commandLine({ arguments: ["FOO=bar", "sh"] }), "'FOO=bar' sh");
  assert.equal(commandLine({ arguments: ["if", "x"] }), "'if' x");
  assert.equal(commandLine({ arguments: ["it's", "x"] }), `'it'\\''s' x`);
});

test("directories and target leaves are distinct rows with no synthetic root", () => {
  const rows = flatten(
    buildTree(["//app/lib:parser", "//apple:server", "//:root"].map((label) => target(label))).root,
    new Set(["app", "app/lib"]),
  );
  // Intermediate directories without their own targets still appear as rows.
  assert.deepEqual(
    rows.map((row) => rowKey(row)),
    ["dir:app@0", "dir:app/lib@1", "tgt://app/lib:parser@2", "dir:apple@0", "tgt://:root@0"],
  );
  const app = rows.find((row) => row.kind === "dir" && row.node.path === "app");
  assert.equal(app?.kind === "dir" && app.node.count, 1);
  const leaf = rows.find((row) => row.kind === "target" && row.target.label === "//:root");
  assert.equal(leaf?.kind === "target" && leaf.depth, 0);
});
