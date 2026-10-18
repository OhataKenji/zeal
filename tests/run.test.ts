import { test } from "node:test";
import assert from "node:assert/strict";
import { runStatus, sectionData, sectionMessage } from "../src/lib/run.ts";
import type { RunRecord } from "../src/lib/types.ts";

const record: RunRecord = {
  id: "one",
  workspace: "/workspace",
  cmd: "build",
  target: "//:app",
  startedAt: 1,
};

test("outcome distinguishes active, stopping, unfinished, succeeded and failed records", () => {
  assert.equal(runStatus(record, "one").label, "● running");
  assert.equal(runStatus(record, "one", true).label, "■ stopping");
  assert.equal(runStatus(record, null).label, "? outcome unknown");
  assert.equal(runStatus(record, "other", true).label, "? outcome unknown");
  assert.equal(runStatus({ ...record, finishedAt: 2, exitCode: 0 }, null).label, "✓ succeeded");
  assert.equal(
    runStatus({ ...record, finishedAt: 2, exitCode: 3 }, null).label,
    "✗ failed (exit 3)",
  );
});

test("stopping never replaces a result the observation already confirmed", () => {
  assert.equal(
    runStatus({ ...record, finishedAt: 2, exitCode: 0 }, record.id, true).label,
    "✓ succeeded",
  );
  assert.equal(
    runStatus({ ...record, finishedAt: 2, exitCode: 3 }, record.id, true).label,
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
  assert.equal(runStatus({ ...record, finishedAt: 2, exitCode: 0 }, null).label, "✓ succeeded");
});

test("status never paints a partial terminal record as succeeded or failed", () => {
  assert.equal(runStatus({ ...record, exitCode: 0 }, null).kind, "unknown");
  assert.equal(runStatus({ ...record, exitCode: 3 }, null).kind, "unknown");
  assert.equal(runStatus({ ...record, finishedAt: 2 }, null).kind, "unknown");
  assert.equal(runStatus(undefined, null).kind, "unknown");
});

test("icon status and description agree when completion races with stop", () => {
  assert.deepEqual(runStatus({ ...record, finishedAt: 2, exitCode: 0 }, record.id, true), {
    kind: "succeeded",
    label: "✓ succeeded",
  });
  assert.deepEqual(runStatus({ ...record, finishedAt: 2, exitCode: 3 }, record.id, true), {
    kind: "failed",
    label: "✗ failed (exit 3)",
  });
  assert.deepEqual(runStatus(record, record.id), { kind: "running", label: "● running" });
  assert.deepEqual(runStatus(record, record.id, true), { kind: "stopping", label: "■ stopping" });
});
