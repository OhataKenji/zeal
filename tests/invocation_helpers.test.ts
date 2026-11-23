import { test } from "node:test";
import assert from "node:assert/strict";
import { rerunArgs } from "../src/lib/invocation.ts";

test("rerunArgs allows build/test/run with valid patterns and rejects invalid ones", () => {
  assert.deepEqual(rerunArgs({ id: "1", cmd: "test", pattern: "//app:all" }), {
    cmd: "test",
    pattern: "//app:all",
  });
  assert.equal(rerunArgs({ id: "1", cmd: "query", pattern: "//..." }), null);
  assert.equal(rerunArgs({ id: "1", cmd: "build", pattern: "-//app:bad" }), null);
  assert.equal(rerunArgs({ id: "1", cmd: "build" }), null);
});
