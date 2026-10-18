import { test } from "node:test";
import assert from "node:assert/strict";
import { outcome, sectionData, sectionMessage } from "../src/lib/run.ts";
import type { RunRecord } from "../src/lib/types.ts";

const record: RunRecord = {
  id: "one",
  workspace: "/workspace",
  cmd: "build",
  target: "//:app",
  startedAt: 1,
};

test("outcome distinguishes active, stopping, unfinished, succeeded and failed records", () => {
  assert.equal(outcome(record, "one"), "● running");
  assert.equal(outcome(record, "one", true), "■ stopping");
  assert.equal(outcome(record, null), "? outcome unknown");
  assert.equal(outcome(record, "other", true), "? outcome unknown");
  assert.equal(outcome({ ...record, finishedAt: 2, exitCode: 0 }, null), "✓ succeeded");
  assert.equal(outcome({ ...record, finishedAt: 2, exitCode: 3 }, null), "✗ failed (exit 3)");
});

test("stopping never replaces a result the observation already confirmed", () => {
  assert.equal(outcome({ ...record, finishedAt: 2, exitCode: 0 }, record.id, true), "✓ succeeded");
  assert.equal(
    outcome({ ...record, finishedAt: 2, exitCode: 3 }, record.id, true),
    "✗ failed (exit 3)",
  );
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
  assert.equal(outcome({ ...record, finishedAt: 2, exitCode: 0 }, null), "✓ succeeded");
});
