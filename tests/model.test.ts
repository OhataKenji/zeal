import { test } from "node:test";
import assert from "node:assert/strict";
import { ancestorsOf, buildTree, flatten, relations, splitLabel } from "../src/lib/model.ts";
import type { Snapshot } from "../src/lib/types.ts";

test("target tree lists packages and rules without input or output files", () => {
  const snapshot: Snapshot = {
    meta: { fetchedAt: 0, status: "ok" },
    targets: [
      {
        label: "//app:parser_test",
        pkg: "app",
        name: "parser_test",
        ruleClass: "cc_test",
        location: null,
      },
    ],
    edges: [],
    files: [
      { path: "app/parser_test.cc", owner: "//app:parser_test", role: "srcs" },
      { path: "data/test.json", owner: "//app:parser_test", role: "data" },
      { path: "app/parser_test", owner: "//app:parser_test", role: "out" },
    ],
  };
  const rows = flatten(buildTree(snapshot), new Set(["app"]));
  assert.deepEqual(
    rows.map((row) => ({ kind: row.kind, text: row.text })),
    [
      { kind: "dir", text: "app/" },
      { kind: "rule", text: ":parser_test" },
    ],
  );
  assert.equal(rows[1].label, "//app:parser_test");
  assert.equal(rows[1].ruleClass, "cc_test");
});

test("revealing a target expands every ancestor and keeps other branches open", () => {
  const snapshot: Snapshot = {
    meta: { fetchedAt: 0, status: "ok" },
    targets: ["//app/lib/deep:parser", "//other:lib", "//:root"].map((label) => ({
      label,
      ...splitLabel(label),
      ruleClass: "cc_library",
      location: null,
    })),
    edges: [],
    files: [],
  };
  const tree = buildTree(snapshot);
  const expanded = new Set(["other"]);
  const label = "//app/lib/deep:parser";
  assert.ok(!flatten(tree, expanded).some((row) => row.label === label));

  for (const path of ancestorsOf(splitLabel(label).pkg)) {
    expanded.add(path);
  }

  assert.deepEqual([...expanded], ["other", "app", "app/lib", "app/lib/deep"]);
  assert.ok(flatten(tree, expanded).some((row) => row.label === label));
  assert.deepEqual(ancestorsOf(splitLabel("//:root").pkg), []);
  assert.ok(flatten(tree, new Set()).some((row) => row.label === "//:root"));
});

test("reverse dependencies group unique attributes under each direct user", () => {
  const snapshot: Snapshot = {
    meta: { fetchedAt: 0, status: "ok" },
    targets: [
      { label: "//app:lib", pkg: "app", name: "lib", ruleClass: "cc_library", location: null },
    ],
    edges: [
      { from: "//app:z", to: "//app:lib", attr: "srcs" },
      { from: "//app:a", to: "//app:lib", attr: "deps" },
      { from: "//app:z", to: "//app:lib", attr: "hdrs" },
      { from: "//app:z", to: "//app:lib", attr: "srcs" },
      { from: "//app:indirect", to: "//app:z", attr: "deps" },
    ],
    files: [],
  };
  assert.deepEqual(relations(snapshot, "//app:lib")?.usedBy, [
    { label: "//app:a", attrs: ["deps"] },
    { label: "//app:z", attrs: ["hdrs", "srcs"] },
  ]);
});

test("forward attributes retain rule and file inputs under their original names", () => {
  const snapshot: Snapshot = {
    meta: { fetchedAt: 0, status: "ok" },
    targets: [
      {
        label: "//app:generated",
        pkg: "app",
        name: "generated",
        ruleClass: "genrule",
        location: null,
      },
    ],
    edges: [
      { from: "//app:generated", to: "//app:inputs", attr: "srcs" },
      { from: "//app:generated", to: "//app:inputs", attr: "srcs" },
      { from: "//app:generated", to: "//tools:compiler", attr: "tools" },
      { from: "//other:target", to: "//app:inputs", attr: "deps" },
    ],
    files: [
      { owner: "//app:generated", path: "app/input.txt", role: "srcs" },
      { owner: "//app:generated", path: "app/runtime.txt", role: "data" },
      { owner: "//app:generated", path: "app/helper.sh", role: "tools" },
      { owner: "//app:generated", path: "app/generated.txt", role: "out" },
      { owner: "//other:target", path: "other/input.txt", role: "srcs" },
    ],
  };
  const result = relations(snapshot, "//app:generated");
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
