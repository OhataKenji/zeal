import assert from "node:assert/strict";
import test from "node:test";
import { rerunState, summarizeRunners } from "../src/lib/invocation.ts";
import type { InvocationDetail } from "../src/lib/types.ts";

test("summarizeRunners excludes total row and computes cache hit percentage", () => {
  assert.deepEqual(
    summarizeRunners([
      { name: "total", count: 10 },
      { name: "internal", count: 2 },
      { name: "remote cache hit", count: 6 },
      { name: "linux-sandbox", count: 2 },
      { name: "zero", count: 0 },
    ]),
    {
      total: 8,
      cached: 6,
      hitPct: 75,
      runners: [
        { name: "remote cache hit", count: 6, cached: true },
        { name: "linux-sandbox", count: 2, cached: false },
      ],
    },
  );
  assert.equal(
    summarizeRunners([
      { name: "total", count: 3 },
      { name: "internal", count: 3 },
    ]),
    null,
  );
  assert.equal(summarizeRunners([]), null);
  assert.equal(summarizeRunners(undefined), null);
});

function savedDetail(id: string, rerunFlags?: string): InvocationDetail {
  return {
    record: { id, cmd: "build", pattern: "//:mode_sensitive" },
    bep: { status: "ok", data: { targets: [], tests: [], rerunFlags, truncated: false } },
    log: { status: "ok", data: "" },
  };
}

test("rerunState models History Re-run transitions explicitly", () => {
  const base = {
    record: { id: "1", cmd: "build", pattern: "//:mode_sensitive" },
    execution: undefined,
    diagnostic: null,
    detail: savedDetail("1", "--define=qa_mode=enabled"),
    detailLoading: false,
    detailError: null,
    runnable: true,
  };

  assert.deepEqual(rerunState(base), {
    kind: "ready",
    cmd: "build",
    pattern: "//:mode_sensitive",
    flags: "--define=qa_mode=enabled",
  });
  assert.deepEqual(rerunState({ ...base, detail: savedDetail("1", "") }), {
    kind: "ready",
    cmd: "build",
    pattern: "//:mode_sensitive",
    flags: "",
  });
  assert.deepEqual(rerunState({ ...base, detailLoading: true }), { kind: "loading" });
  assert.deepEqual(rerunState({ ...base, detailError: "read failed" }), { kind: "unavailable" });
  assert.deepEqual(rerunState({ ...base, detail: savedDetail("other", "--foo") }), {
    kind: "unavailable",
  });
  assert.deepEqual(rerunState({ ...base, detail: savedDetail("1", undefined) }), {
    kind: "unavailable",
  });
  assert.deepEqual(rerunState({ ...base, runnable: false }), { kind: "unavailable" });
  assert.deepEqual(rerunState({ ...base, record: { id: "1", cmd: "query", pattern: "//..." } }), {
    kind: "unavailable",
  });
  assert.deepEqual(
    rerunState({ ...base, record: { id: "1", cmd: "build", pattern: "-//app:bad" } }),
    { kind: "unavailable" },
  );
  assert.deepEqual(
    rerunState({
      ...base,
      execution: {
        id: "1",
        workspace: "/ws",
        cmd: "build",
        pattern: "//:mode_sensitive",
        startedAt: 1,
        stoppable: true,
        stopping: false,
        console: true,
      },
    }),
    { kind: "unavailable" },
  );
  assert.deepEqual(
    rerunState({
      ...base,
      detail: null,
      diagnostic: {
        id: "1",
        workspace: "/ws",
        cmd: "build",
        pattern: "//:mode_sensitive",
        startedAt: 1,
        processExitCode: 1,
        output: "",
      },
    }),
    { kind: "ready", cmd: "build", pattern: "//:mode_sensitive", flags: "" },
  );
});
