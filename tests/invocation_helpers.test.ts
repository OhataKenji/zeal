import { test } from "node:test";
import assert from "node:assert/strict";
import { rerunArgs, summarizeRunners } from "../src/lib/invocation.ts";

test("summarizeRunners excludes total row and computes cache hit percentage", () => {
  const sum = summarizeRunners([
    { name: "total", count: 42 },
    { name: "disk cache hit", count: 28 },
    { name: "remote cache hit", count: 6 },
    { name: "darwin-sandbox", count: 8 },
  ]);
  assert.deepEqual(sum, {
    total: 42,
    cached: 34,
    hitPct: 81,
    runners: [
      { name: "disk cache hit", count: 28, cached: true },
      { name: "remote cache hit", count: 6, cached: true },
      { name: "darwin-sandbox", count: 8, cached: false },
    ],
  });
  assert.equal(summarizeRunners([]), null);
  assert.equal(summarizeRunners([{ name: "total", count: 5 }]), null);
});

test("rerunArgs allows build/test/run with valid patterns and rejects invalid ones", () => {
  assert.deepEqual(rerunArgs({ id: "1", cmd: "test", pattern: "//app:all" }), {
    cmd: "test",
    pattern: "//app:all",
  });
  assert.equal(rerunArgs({ id: "1", cmd: "query", pattern: "//..." }), null);
  assert.equal(rerunArgs({ id: "1", cmd: "build", pattern: "-//app:bad" }), null);
  assert.equal(rerunArgs({ id: "1", cmd: "build" }), null);
});
