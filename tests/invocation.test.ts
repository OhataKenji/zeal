import { test } from "node:test";
import assert from "node:assert/strict";
import {
  invocationStatus,
  localDay,
  matchesHistoryFilter,
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
    stoppable: true,
    stopping: false,
    console: true,
    ...over,
  };
}

test("a live owned entry always reads running or stopping", () => {
  assert.equal(invocationStatus(RECORD, owned()).label, "● running");
  assert.equal(invocationStatus(RECORD, owned({ stopping: true })).label, "■ stopping");
  // A BEP result that already arrived cannot outrank the live process: a
  // `run` program may still be going after its build succeeded.
  const runDone = { ...RECORD, cmd: "run", finishedAt: 2, exitCode: 0 };
  assert.equal(invocationStatus(runDone, owned({ cmd: "run" })).label, "● running");
  assert.equal(
    invocationStatus(runDone, owned({ cmd: "run", stopping: true })).label,
    "■ stopping",
  );
  assert.equal(invocationStatus(RECORD).label, "? outcome unknown");
});

test("a finished run reports its build result, never the program's", () => {
  const run = { ...RECORD, cmd: "run" };
  assert.equal(invocationStatus({ ...run, finishedAt: 2, exitCode: 0 }).label, "✓ build succeeded");
  assert.equal(
    invocationStatus({ ...run, finishedAt: 2, exitCode: 3 }).label,
    "✗ build failed (exit 3)",
  );
  assert.equal(invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }).label, "✓ succeeded");
  assert.equal(
    invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 3 }).label,
    "✗ failed (exit 3)",
  );
});

test("a rejected local process is a failure without claiming a BEP build result", () => {
  const run = { ...RECORD, cmd: "run" };
  assert.deepEqual(invocationStatus(run, undefined, 9), {
    kind: "failed",
    label: "✗ process failed (exit 9)",
  });
  assert.deepEqual(invocationStatus(run, undefined, 8), {
    kind: "failed",
    label: "✗ process failed (exit 8)",
  });
  assert.equal(invocationStatus(RECORD, undefined, 8).kind, "interrupted");
  assert.equal(invocationStatus(run, undefined, 0).kind, "unknown");
  assert.equal(invocationStatus(run, undefined, null).kind, "unknown");
});

test("History filters distinguish process failures, running owners and saved results", () => {
  const record = owned();
  const diagnostic = { ...record, processExitCode: 9, output: "Another command is running." };
  const rows = [
    { record, diagnostic },
    { record, diagnostic: { ...diagnostic, processExitCode: null } },
    { record, execution: record },
    { record, execution: owned({ stopping: true }) },
    { record: { ...RECORD, exitCode: 1 } },
    { record: { ...RECORD, exitCode: 0 } },
    { record: { ...RECORD, exitCode: 8 } },
  ];
  assert.deepEqual(
    rows.filter((row) => matchesHistoryFilter(row, "all")),
    rows,
  );
  assert.deepEqual(
    rows.filter((row) => matchesHistoryFilter(row, "failed")),
    [rows[0], rows[4]],
  );
  assert.deepEqual(
    rows.filter((row) => matchesHistoryFilter(row, "running")),
    [rows[2], rows[3]],
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

test("pending observations carry no data and read as neutral", () => {
  assert.equal(sectionData({ status: "pending" }), null);
  assert.equal(sectionMessage({ status: "pending" }), "No build events available yet.");
  assert.equal(sectionNoticeTone({ status: "pending" }), "neutral");
});

test("build exit determines outcome even when its finish time was not recorded", () => {
  assert.equal(invocationStatus({ ...RECORD, exitCode: 0 }).kind, "succeeded");
  assert.equal(invocationStatus({ ...RECORD, exitCode: 3 }).kind, "failed");
  assert.equal(invocationStatus({ ...RECORD, exitCode: 8 }).kind, "interrupted");
});

test("missing build exit remains unknown even when its finish time was recorded", () => {
  assert.equal(invocationStatus({ ...RECORD, finishedAt: 2 }).kind, "unknown");
  assert.equal(invocationStatus(undefined).kind, "unknown");
});

test("an unobserved invocation has no invented input or result", () => {
  assert.deepEqual(invocationStatus({ id: "reserved" }), {
    kind: "unknown",
    label: "? outcome unknown",
  });
  assert.equal(invocationStatus(undefined, owned({ id: "reserved" })).kind, "running");
});

test("icon status and description agree for live and finished invocations", () => {
  assert.deepEqual(invocationStatus(RECORD, owned()), {
    kind: "running",
    label: "● running",
  });
  assert.deepEqual(invocationStatus(RECORD, owned({ stopping: true })), {
    kind: "stopping",
    label: "■ stopping",
  });
  assert.deepEqual(invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 0 }), {
    kind: "succeeded",
    label: "✓ succeeded",
  });
  assert.deepEqual(invocationStatus({ ...RECORD, finishedAt: 2, exitCode: 3 }), {
    kind: "failed",
    label: "✗ failed (exit 3)",
  });
});

test("localDay formats local YYYY-MM-DD calendar dates and rejects missing timestamps", () => {
  assert.equal(localDay(undefined), null);
  assert.equal(localDay(Number.NaN), null);
  assert.equal(localDay(new Date(2026, 2, 5, 23, 59, 59).getTime()), "2026-03-05");
});
