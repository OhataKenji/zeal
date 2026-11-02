import { test } from "node:test";
import assert from "node:assert/strict";
import {
  invocationStatus,
  sectionData,
  sectionMessage,
  sectionNoticeTone,
} from "../src/lib/invocation.ts";
import type { InvocationRecord } from "../src/lib/types.ts";

const RECORD: InvocationRecord = {
  id: "one",
  workspace: "/workspace",
  cmd: "build",
  pattern: "//:app",
  startedAt: 1,
};

test("outcome distinguishes active, stopping, unfinished, succeeded and failed records", () => {
  assert.equal(invocationStatus(RECORD, "one").label, "● running");
  assert.equal(invocationStatus(RECORD, "one", true).label, "■ stopping");
  assert.equal(invocationStatus(RECORD, null).label, "? outcome unknown");
  assert.equal(invocationStatus(RECORD, "other", true).label, "? outcome unknown");
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }, null).label,
    "✓ succeeded",
  );
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 3 }, null).label,
    "✗ failed (exit 3)",
  );
});

test("stopping never replaces a result the observation already confirmed", () => {
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }, RECORD.id, true).label,
    "✓ succeeded",
  );
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 3 }, RECORD.id, true).label,
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
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }, null).label,
    "✓ succeeded",
  );
});

test("section notices distinguish broken reads, partial data, and absent records", () => {
  assert.equal(sectionNoticeTone({ status: "read_error", note: "permission denied" }), "error");
  assert.equal(sectionNoticeTone({ status: "parse_error", note: "invalid JSON" }), "error");
  assert.equal(
    sectionNoticeTone({ status: "incomplete", data: "partial", note: "truncated" }),
    "warning",
  );
  assert.equal(sectionNoticeTone({ status: "missing" }), "neutral");
  assert.equal(sectionNoticeTone({ status: "not_available", note: "still running" }), "neutral");
  assert.equal(sectionNoticeTone({ status: "ok", data: "complete" }), "neutral");
});

test("build exit determines outcome even when its finish time was not recorded", () => {
  assert.equal(invocationStatus({ ...RECORD, exitCode: 0 }, null).kind, "succeeded");
  assert.equal(invocationStatus({ ...RECORD, exitCode: 3 }, null).kind, "failed");
});

test("missing build exit remains unknown even when its finish time was recorded", () => {
  assert.equal(invocationStatus({ ...RECORD, finishedAt: 2 }, null).kind, "unknown");
  assert.equal(invocationStatus(undefined, null).kind, "unknown");
});

test("an unobserved invocation has no invented input or result", () => {
  assert.deepEqual(invocationStatus({ id: "reserved" }, null), {
    kind: "unknown",
    label: "? outcome unknown",
  });
  assert.equal(invocationStatus({ id: "reserved" }, "reserved").kind, "running");
});

test("icon status and description agree when completion races with stop", () => {
  assert.deepEqual(invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }, RECORD.id, true), {
    kind: "succeeded",
    label: "✓ succeeded",
  });
  assert.deepEqual(invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 3 }, RECORD.id, true), {
    kind: "failed",
    label: "✗ failed (exit 3)",
  });
  assert.deepEqual(invocationStatus(RECORD, RECORD.id), { kind: "running", label: "● running" });
  assert.deepEqual(invocationStatus(RECORD, RECORD.id, true), {
    kind: "stopping",
    label: "■ stopping",
  });
});
