import { test } from "node:test";
import assert from "node:assert/strict";
import {
  invocationStatus,
  sectionData,
  sectionMessage,
  sectionNoticeTone,
} from "../src/lib/invocation.ts";
import type { InvocationRecord, OwnedInvocation } from "../src/lib/types.ts";

const RECORD: InvocationRecord = {
  id: "one",
  workspace: "/workspace",
  cmd: "build",
  pattern: "//:app",
  startedAt: 1,
};

function owned(over: Partial<OwnedInvocation> = {}): OwnedInvocation {
  return {
    id: RECORD.id,
    workspace: "/workspace",
    cmd: "build",
    pattern: "//:app",
    startedAt: 1,
    running: true,
    stoppable: true,
    stopping: false,
    console: true,
    processExitCode: null,
    note: null,
    ...over,
  };
}

test("outcome distinguishes active, stopping, unfinished, succeeded and failed records", () => {
  assert.equal(invocationStatus(RECORD, owned()).label, "● running");
  assert.equal(invocationStatus(RECORD, owned({ stopping: true })).label, "■ stopping");
  assert.equal(invocationStatus(RECORD).label, "? outcome unknown");
  assert.equal(
    invocationStatus(RECORD, owned({ running: false, stoppable: false })).label,
    "? outcome unknown",
  );
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }, owned()).label,
    "✓ succeeded",
  );
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 3 }, owned()).label,
    "✗ failed (exit 3)",
  );
});

test("stopping never replaces a result the observation already confirmed", () => {
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }, owned({ stopping: true })).label,
    "✓ succeeded",
  );
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 3 }, owned({ stopping: true })).label,
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
  assert.equal(invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }).label, "✓ succeeded");
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
  assert.equal(invocationStatus({ ...RECORD, exitCode: 0 }).kind, "succeeded");
  assert.equal(invocationStatus({ ...RECORD, exitCode: 3 }).kind, "failed");
});

test("missing build exit remains unknown even when its finish time was recorded", () => {
  assert.equal(invocationStatus({ ...RECORD, finishedAt: 2 }).kind, "unknown");
  assert.equal(invocationStatus(undefined).kind, "unknown");
  // A finished owned process without BEP data reports its process exit separately.
  assert.equal(
    invocationStatus(undefined, owned({ running: false, stoppable: false, processExitCode: 7 }))
      .kind,
    "unknown",
  );
});

test("an unobserved invocation has no invented input or result", () => {
  assert.deepEqual(invocationStatus({ id: "reserved" }), {
    kind: "unknown",
    label: "? outcome unknown",
  });
  assert.equal(invocationStatus(undefined, owned({ id: "reserved" })).kind, "running");
});

test("icon status and description agree when completion races with stop", () => {
  assert.deepEqual(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }, owned({ stopping: true })),
    {
      kind: "succeeded",
      label: "✓ succeeded",
    },
  );
  assert.deepEqual(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 3 }, owned({ stopping: true })),
    {
      kind: "failed",
      label: "✗ failed (exit 3)",
    },
  );
  assert.deepEqual(invocationStatus(RECORD, owned()), {
    kind: "running",
    label: "● running",
  });
  assert.deepEqual(invocationStatus(RECORD, owned({ stopping: true })), {
    kind: "stopping",
    label: "■ stopping",
  });
});
