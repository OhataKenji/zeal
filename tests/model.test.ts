import { test } from "node:test";
import assert from "node:assert/strict";
import {
  ancestorsOf,
  buildTree,
  commandLine,
  flatten,
  relations,
  shellQuote,
  splitLabel,
} from "../src/lib/model.ts";
import type { TargetDetail, TargetIndex } from "../src/lib/types.ts";

test("package tree counts targets per subtree", () => {
  const snapshot: TargetIndex = {
    meta: { fetchedAt: 0, status: "ok" },
    targets: [
      {
        label: "//app:parser_test",
        pkg: "app",
        name: "parser_test",
        ruleClass: "cc_test",
        location: null,
      },
      {
        label: "//app/lib:helper",
        pkg: "app/lib",
        name: "helper",
        ruleClass: "cc_library",
        location: null,
      },
      {
        label: "//:root_target",
        pkg: "",
        name: "root_target",
        ruleClass: "cc_library",
        location: null,
      },
    ],
  };
  const tree = buildTree(snapshot);
  assert.equal(tree.count, 3);
  assert.equal(tree.dirs.find((d) => d.path === "app")?.count, 2);
  assert.equal(tree.dirs.find((d) => d.path === "app")?.dirs[0]?.count, 1);

  const rows = flatten(tree, new Set(["app"]));
  assert.deepEqual(
    rows.map((row) => ({ path: row.path, name: row.name, depth: row.depth })),
    [
      { path: "app", name: "app/", depth: 0 },
      { path: "app/lib", name: "lib/", depth: 1 },
    ],
  );
  assert.equal(rows[0].count, 2);
  assert.equal(rows[1].count, 1);
  assert.ok(rows.every((row) => !("kind" in row) && !("label" in row)));
});

test("expanding a package reveals its row without opening its children", () => {
  const snapshot: TargetIndex = {
    meta: { fetchedAt: 0, status: "ok" },
    targets: ["//app/lib/deep:parser", "//other:lib", "//:root"].map((label) => ({
      label,
      ...splitLabel(label),
      ruleClass: "cc_library",
      location: null,
    })),
  };
  const tree = buildTree(snapshot);
  const expanded = new Set(["other"]);

  // Collapsed branches still show their own row.
  assert.deepEqual(
    flatten(tree, expanded).map((row) => row.path),
    ["app", "other"],
  );

  // Revealing //app/lib/deep opens its ancestors but not the package itself.
  for (const path of ancestorsOf("app/lib/deep").slice(0, -1)) {
    expanded.add(path);
  }
  const paths = flatten(tree, expanded).map((row) => row.path);
  assert.deepEqual(paths, ["app", "app/lib", "app/lib/deep", "other"]);
  assert.deepEqual(ancestorsOf(splitLabel("//:root").pkg), []);
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

test("intermediate package directories appear as rows", () => {
  const index: TargetIndex = {
    meta: { fetchedAt: 0, status: "ok" },
    targets: ["//app/lib:parser", "//apple:server", "//:root"].map((label) => ({
      label,
      ...splitLabel(label),
      ruleClass: "cc_library",
      location: null,
    })),
  };
  const rows = flatten(buildTree(index), new Set(["app"]));
  // Intermediate directories without their own targets still appear as rows.
  assert.deepEqual(
    rows.map((row) => row.path),
    ["app", "app/lib", "apple"],
  );
  assert.equal(rows.find((row) => row.path === "app")?.count, 1);
  assert.equal(rows.find((row) => row.path === "app/lib")?.count, 1);
  assert.equal(rows.find((row) => row.path === "apple")?.count, 1);
});
