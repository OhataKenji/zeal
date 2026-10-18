import { test } from "node:test";
import assert from "node:assert/strict";
import { buildTree, flatten } from "../src/lib/model.ts";
import type { Snapshot } from "../src/lib/types.ts";

test("tree rows keep rule kind separate from files that navigate to that rule", () => {
  const snapshot: Snapshot = {
    meta: { fetchedAt: 0, status: "ok" },
    targets: [
      {
        label: "//app:parser_test",
        pkg: "app",
        name: "parser_test",
        ruleClass: "java_test",
        location: null,
      },
    ],
    edges: [],
    files: [{ path: "app/ParserTest.java", owner: "//app:parser_test", role: "srcs" }],
  };
  const rows = flatten(buildTree(snapshot), new Set(["app"]));
  const rule = rows.find((row) => row.kind === "rule")!;
  const file = rows.find((row) => row.kind === "file")!;
  assert.equal(rule.ruleClass, "java_test");
  assert.equal(file.ruleClass, undefined);
  assert.equal(rule.label, file.label);
  assert.equal(file.text, "ParserTest.java");
});
