import { test } from "node:test";
import assert from "node:assert/strict";
import {
  isFailedTestStatus,
  normalizeTargetLabel,
  rerunArgs,
  summarizeRunners,
} from "../src/lib/invocation.ts";

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
  assert.deepEqual(
    summarizeRunners([
      { name: "total", count: 10 },
      { name: "disk cache hit", count: 9 },
      { name: "internal", count: 1 },
    ]),
    {
      total: 9,
      cached: 9,
      hitPct: 100,
      runners: [{ name: "disk cache hit", count: 9, cached: true }],
    },
  );
});

test("rerunArgs allows build/test/run with valid patterns, inherits explicit flags, and rejects invalid ones", () => {
  assert.deepEqual(rerunArgs({ id: "1", cmd: "test", pattern: "//app:all" }), {
    cmd: "test",
    pattern: "//app:all",
    flags: "",
  });
  assert.deepEqual(
    rerunArgs({ id: "1", cmd: "test", pattern: "//app:all" }, [
      "--bes_backend=grpc://127.0.0.1:4718",
      "--invocation_id=1",
      "--tool_tag=zelith",
      "--config=asan",
      "--test_env=MSG=hello world",
    ]),
    {
      cmd: "test",
      pattern: "//app:all",
      flags: "--config=asan --test_env='MSG=hello world'",
    },
  );
  assert.equal(rerunArgs({ id: "1", cmd: "query", pattern: "//..." }), null);
  assert.equal(rerunArgs({ id: "1", cmd: "build", pattern: "-//app:bad" }), null);
  assert.equal(rerunArgs({ id: "1", cmd: "build" }), null);
});

test("normalizeTargetLabel strips main-repo @@ prefix and isFailedTestStatus flags non-passing tests", () => {
  assert.equal(normalizeTargetLabel("@@//pkg:unit_test"), "//pkg:unit_test");
  assert.equal(normalizeTargetLabel("//pkg:unit_test"), "//pkg:unit_test");
  assert.equal(normalizeTargetLabel("@@rules_cc//pkg:unit_test"), "@@rules_cc//pkg:unit_test");

  assert.equal(isFailedTestStatus("PASSED"), false);
  assert.equal(isFailedTestStatus("FAILED"), true);
  assert.equal(isFailedTestStatus("TIMEOUT"), true);
  assert.equal(isFailedTestStatus("FLAKY"), true);
});
