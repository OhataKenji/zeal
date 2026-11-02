import { test } from "node:test";
import assert from "node:assert/strict";
import { ancestorsOf, buildTree, flatten, relations, splitLabel } from "../src/lib/model.ts";
import type { TargetDetail, TargetIndex } from "../src/lib/types.ts";

test("target tree lists packages and targets without input or output files", () => {
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
    ],
  };
  const rows = flatten(buildTree(snapshot), new Set(["app"]));
  assert.deepEqual(
    rows.map((row) => ({ kind: row.kind, text: row.text })),
    [
      { kind: "dir", text: "app/" },
      { kind: "target", text: ":parser_test" },
    ],
  );
  assert.equal(rows[1].label, "//app:parser_test");
  assert.equal(rows[1].ruleClass, "cc_test");
});

test("revealing a target expands every ancestor and keeps other branches open", () => {
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
