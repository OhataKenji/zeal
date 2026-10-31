import { test } from "node:test";
import assert from "node:assert/strict";
import { previewAnalytics } from "../src/preview/analytics.ts";
import type { AnalyticsFilter } from "../src/lib/analytics.ts";

const NOW = new Date(2026, 2, 30, 12).getTime();
const PERIOD = { start: "2026-03-30", end: "2026-03-31" };

test("target selection limits daily counts but preserves the ranking and untimed runs", () => {
  const records = [
    { cmd: "build", target: "//:a", startedAt: NOW, finishedAt: NOW + 100, exitCode: 0 },
    { cmd: "build", target: "//:a", startedAt: NOW, exitCode: 1 },
    { cmd: "test", target: "//:b", startedAt: NOW, finishedAt: NOW + 500, exitCode: 8 },
    { cmd: "run", target: "//:a", startedAt: NOW, finishedAt: NOW + 900, exitCode: 0 },
    { cmd: "build", target: "//:a", startedAt: NOW },
  ];
  const before = structuredClone(records);
  const result = previewAnalytics(records, { ...PERIOD, target: "//:a" }, NOW);
  assert.deepEqual(result.days, [
    {
      day: "2026-03-30",
      count: 2,
      timed: 1,
      ok: 1,
      failed: 1,
      interrupted: 0,
      totalMs: 100,
      p50Ms: 100,
      p95Ms: 100,
    },
    {
      day: "2026-03-31",
      count: 0,
      timed: 0,
      ok: 0,
      failed: 0,
      interrupted: 0,
      totalMs: 0,
      p50Ms: 0,
      p95Ms: 0,
    },
  ]);
  assert.deepEqual(result.targets, [
    { target: "//:b", runs: 1, totalMs: 500, p50Ms: 500, p95Ms: 500 },
    { target: "//:a", runs: 2, totalMs: 100, p50Ms: 100, p95Ms: 100 },
  ]);
  assert.deepEqual(records, before);
});

test("command and outcome selection apply to both daily results and ranking", () => {
  const result = previewAnalytics(
    [
      { cmd: "build", target: "//:a", startedAt: NOW, finishedAt: NOW + 100, exitCode: 8 },
      { cmd: "test", target: "//:b", startedAt: NOW, finishedAt: NOW + 500, exitCode: 8 },
      { cmd: "test", target: "//:c", startedAt: NOW, finishedAt: NOW + 900, exitCode: 0 },
    ],
    { ...PERIOD, cmd: "test", outcome: "interrupted" },
    NOW,
  );
  assert.equal(result.days[0].interrupted, 1);
  assert.equal(result.days[0].count, 1);
  assert.deepEqual(
    result.targets.map((row) => row.target),
    ["//:b"],
  );
});

test("timings clamp negative durations and use nearest-rank percentiles", () => {
  const result = previewAnalytics(
    [
      { cmd: "build", target: "//:a", startedAt: NOW, finishedAt: NOW - 1, exitCode: 0 },
      { cmd: "build", target: "//:a", startedAt: NOW, finishedAt: NOW + 100, exitCode: 0 },
      { cmd: "build", target: "//:a", startedAt: NOW, finishedAt: NOW + 200, exitCode: 0 },
      { cmd: "build", target: "//:a", startedAt: NOW, finishedAt: NOW + 300, exitCode: 0 },
    ],
    PERIOD,
    NOW,
  );
  assert.deepEqual(result.targets, [
    { target: "//:a", runs: 4, totalMs: 600, p50Ms: 100, p95Ms: 300 },
  ]);
});

test("missing dates default to thirty local calendar days without adding optional filters", () => {
  const result = previewAnalytics([], {}, NOW);
  assert.deepEqual(result.filter, { start: "2026-03-01", end: "2026-03-30" });
  assert.equal(result.today, "2026-03-30");
  assert.equal(result.days.length, 30);
});

test("calendar validation accepts leap days and early years", () => {
  assert.equal(
    previewAnalytics([], { start: "2024-01-01", end: "2024-12-31" }, NOW).days.length,
    366,
  );
  assert.equal(
    previewAnalytics([], { start: "0000-02-29", end: "0000-02-29" }, NOW).days[0].day,
    "0000-02-29",
  );
  assert.equal(
    previewAnalytics([], { start: "2011-12-30", end: "2011-12-30" }, NOW).days.length,
    1,
  );
});

test("invalid calendars and excessive or reversed periods are rejected", () => {
  assert.throws(
    () => previewAnalytics([], { start: "2025-02-29", end: "2025-03-01" }, NOW),
    /invalid analytics filter/,
  );
  assert.throws(
    () => previewAnalytics([], { start: "2026-3-01", end: "2026-03-01" }, NOW),
    /invalid analytics filter/,
  );
  assert.throws(
    () => previewAnalytics([], { start: "2024-01-01", end: "2025-01-01" }, NOW),
    /invalid analytics filter/,
  );
  assert.throws(
    () => previewAnalytics([], { start: "2026-03-31", end: "2026-03-30" }, NOW),
    /invalid analytics filter/,
  );
});

test("unsupported filters and targets exceeding the UTF-8 byte limit are rejected", () => {
  assert.throws(
    () => previewAnalytics([], { ...PERIOD, cmd: "run" } as unknown as AnalyticsFilter, NOW),
    /invalid analytics filter/,
  );
  assert.throws(
    () =>
      previewAnalytics([], { ...PERIOD, outcome: "unknown" } as unknown as AnalyticsFilter, NOW),
    /invalid analytics filter/,
  );
  assert.throws(
    () => previewAnalytics([], { ...PERIOD, target: "é".repeat(32769) }, NOW),
    /invalid analytics filter/,
  );
  assert.equal(previewAnalytics([], { ...PERIOD, target: "é".repeat(32768) }, NOW).days.length, 2);
});
