import { test } from "node:test";
import assert from "node:assert/strict";
import {
  ancestorsOf,
  buildTree,
  commandLine,
  isTestRule,
  packageContents,
  relations,
  shellQuote,
  splitLabel,
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
  // packageContents keeps the //path heading separate from the :all action target.
  const contents = packageContents(app);
  assert.equal(contents.name, "//app");
  assert.equal(contents.pattern, "//app:all");
});

test("ancestorsOf includes the package but not the root", () => {
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
