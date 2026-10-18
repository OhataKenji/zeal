import { test } from "node:test";
import assert from "node:assert/strict";
import { outcome, sectionData, sectionMessage } from "../src/lib/run.ts";
import type { RunRecord } from "../src/lib/types.ts";

const record: RunRecord = {
  id: "one",
  workspace: "/workspace",
  workspaceId: "workspace-a",
  outcome: "ok",
  bazelElapsedMs: null,
  logTruncated: false,
  cmd: "build",
  target: "//:app",
  startedAt: 1,
};

test("immutable outcome remains known without a valid finish timestamp", () => {
  assert.equal(outcome(record), "✓ succeeded");
  assert.equal(
    outcome({ ...record, outcome: "interrupted", exitCode: 8 }),
    "■ interrupted (exit 8)",
  );
  assert.equal(outcome({ ...record, outcome: "failed", exitCode: 3 }), "✗ failed (exit 3)");
});

test("partial observations retain data and explain the missing tail", () => {
  const section = { status: "incomplete" as const, data: ["accepted"], note: "invalid last event" };
  assert.deepEqual(sectionData(section), ["accepted"]);
  assert.equal(sectionMessage(section), "incomplete — invalid last event");
});

test("missing and unreadable observations remain distinct without changing a known result", () => {
  assert.equal(sectionData({ status: "missing" }), null);
  assert.match(sectionMessage({ status: "missing" }), /^missing/);
  assert.equal(
    sectionMessage({ status: "read_error", note: "permission denied" }),
    "read error — permission denied",
  );
  assert.equal(
    sectionMessage({ status: "parse_error", note: "no Finishing event" }),
    "parse error — no Finishing event",
  );
  assert.equal(
    sectionMessage({ status: "not_available", note: "still running" }),
    "not available — still running",
  );
  assert.equal(outcome({ ...record, finishedAt: 2, exitCode: 0 }), "✓ succeeded");
});
