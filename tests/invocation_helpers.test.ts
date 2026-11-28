import assert from "node:assert/strict";
import test from "node:test";
import { rerunState, summarizeRunners } from "../src/lib/invocation.ts";

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

test("rerunState enforces trusted launch provenance and row readiness", () => {
  const base = {
    record: {
      id: "1",
      cmd: "build",
      pattern: "//:mode_sensitive",
      arguments: "--define=qa_mode=enabled",
    },
    execution: undefined,
    diagnostic: null,
    runnable: true,
  };

  assert.deepEqual(rerunState(base), {
    kind: "ready",
    cmd: "build",
    pattern: "//:mode_sensitive",
    flags: "--define=qa_mode=enabled",
  });
  assert.deepEqual(rerunState({ ...base, record: { ...base.record, arguments: "" } }), {
    kind: "ready",
    cmd: "build",
    pattern: "//:mode_sensitive",
    flags: "",
  });
  // External or forged BES invocations carry no Zelith launch record.
  assert.deepEqual(rerunState({ ...base, record: { ...base.record, arguments: undefined } }), {
    kind: "unavailable",
  });
  assert.deepEqual(rerunState({ ...base, runnable: false }), { kind: "unavailable" });
  assert.deepEqual(
    rerunState({ ...base, record: { id: "1", cmd: "query", pattern: "//...", arguments: "" } }),
    { kind: "unavailable" },
  );
  assert.deepEqual(
    rerunState({
      ...base,
      record: { id: "1", cmd: "build", pattern: "-//app:bad", arguments: "" },
    }),
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
      record: { id: "1", cmd: "build", pattern: "//:mode_sensitive" },
      diagnostic: {
        id: "1",
        workspace: "/ws",
        cmd: "build",
        pattern: "//:mode_sensitive",
        arguments: "--define=qa_mode=enabled",
        startedAt: 1,
        processExitCode: 1,
        output: "",
      },
    }),
    {
      kind: "ready",
      cmd: "build",
      pattern: "//:mode_sensitive",
      flags: "--define=qa_mode=enabled",
    },
  );
});
