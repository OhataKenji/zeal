import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createAnalytics,
  emptyAnalytics,
  fmtMs,
  segments,
  type DailyAnalytics,
} from "../src/lib/analytics.ts";

function harness() {
  let view = emptyAnalytics();
  const calls: Array<{
    args?: Record<string, unknown>;
    resolve: (result: DailyAnalytics) => void;
    reject: (error: Error) => void;
  }> = [];
  const model = createAnalytics(
    {
      invoke: <T>(command: string, args?: Record<string, unknown>) => {
        assert.equal(command, "get_analytics");
        return new Promise<T>((resolve, reject) =>
          calls.push({ args, resolve: (v) => resolve(v as T), reject }),
        );
      },
    },
    (next) => (view = next),
  );
  return {
    model,
    calls,
    get view() {
      return view;
    },
  };
}
const RESULT: DailyAnalytics = {
  filter: { start: "2026-03-01", end: "2026-03-30", cmd: null, outcome: null, target: null },
  today: "2026-03-30",
  days: [
    {
      day: "2026-03-30",
      count: 2,
      timed: 2,
      ok: 1,
      failed: 1,
      interrupted: 0,
      totalMs: 5000,
      p50Ms: 2000,
      p95Ms: 3000,
    },
  ],
  targets: [],
};
function day(count: number, extra: Partial<DailyAnalytics["days"][number]> = {}) {
  return {
    day: "2026-03-30",
    count,
    timed: count,
    ok: count,
    failed: 0,
    interrupted: 0,
    totalMs: 0,
    p50Ms: 0,
    p95Ms: 0,
    ...extra,
  };
}
test("default request uses core calendar and refresh retains all filters", async () => {
  const h = harness();
  const initial = h.model.load();
  assert.deepEqual(h.calls[0].args, {
    filter: { start: null, end: null, cmd: null, outcome: null, target: null },
  });
  h.calls[0].resolve(RESULT);
  await initial;
  const filter = { ...h.view.filter, cmd: "test" as const, outcome: "failed" as const };
  const changed = h.model.load(filter);
  assert.deepEqual(h.calls[1].args, { filter });
  h.calls[1].resolve({ ...RESULT, filter });
  await changed;
  const refreshed = h.model.load();
  assert.deepEqual(h.calls[2].args, { filter });
  h.calls[2].resolve({ ...RESULT, filter });
  await refreshed;
  assert.deepEqual(h.view.filter, filter);
});
test("late filters, errors and disposed workspace requests cannot replace the latest view", async () => {
  const h = harness();
  const old = h.model.load();
  const latest = h.model.load({ ...RESULT.filter, cmd: "test" });
  h.calls[1].resolve({ ...RESULT, days: [day(9)] });
  await latest;
  h.calls[0].reject(new Error("stale failure"));
  await old;
  assert.equal(h.view.data?.days[0].count, 9);
  assert.equal(h.view.error, null);
  const disposed = h.model.load();
  h.model.dispose();
  h.calls[2].resolve(RESULT);
  await disposed;
  // The disposed response never lands; the last applied data stays put.
  assert.equal(h.view.data?.days[0].count, 9);
});
test("reloading keeps the previous charts until the response lands", async () => {
  const h = harness();
  const initial = h.model.load();
  h.calls[0].resolve(RESULT);
  await initial;
  const reloading = h.model.load({ ...RESULT.filter, cmd: "test" });
  assert.equal(h.view.loading, true);
  assert.equal(h.view.data?.days[0].count, 2);
  h.calls[1].reject(new Error("database busy"));
  await reloading;
  assert.equal(h.view.data, null);
  assert.match(h.view.error ?? "", /busy/);
});
test("fmtMs carries rounded seconds into the larger unit", () => {
  assert.equal(fmtMs(999), "999ms");
  assert.equal(fmtMs(9_420), "9.4s");
  assert.equal(fmtMs(9_950), "10s");
  assert.equal(fmtMs(42_000), "42s");
  assert.equal(fmtMs(119_950), "2m 0s");
  assert.equal(fmtMs(3_600_000), "1h 0m");
});
test("segments splits a series at days without durations", () => {
  assert.deepEqual(segments([1, null, 2], 2, 10), ["60,120", "80,40"]);
  assert.deepEqual(segments([], 2, 10), []);
});
test("errors remove stale data and retry preserves requested conditions", async () => {
  const h = harness();
  const failed = h.model.load(RESULT.filter);
  h.calls[0].reject(new Error("database busy"));
  await failed;
  assert.match(h.view.error ?? "", /busy/);
  assert.equal(h.view.loading, false);
  const retry = h.model.load();
  assert.deepEqual(h.calls[1].args, h.calls[0].args);
  h.calls[1].resolve(RESULT);
  await retry;
  assert.equal(h.view.error, null);
  assert.equal(h.view.data?.days[0].count, 2);
});
