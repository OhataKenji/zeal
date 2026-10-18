import { test } from "node:test";
import assert from "node:assert/strict";
import { createAnalytics, emptyAnalytics, type DailyAnalytics } from "../src/lib/analytics.ts";

function harness() {
  let view = emptyAnalytics();
  const calls: {
    args?: Record<string, unknown>;
    resolve: (result: DailyAnalytics) => void;
    reject: (error: Error) => void;
  }[] = [];
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
const result: DailyAnalytics = {
  filter: { start: "2026-03-01", end: "2026-03-30", cmd: null, outcome: null, day: null },
  today: "2026-03-30",
  days: [{ day: "2026-03-30", count: 2 }],
  total: 2,
  runs: [],
};
test("default request uses core calendar and date drill-down retains all filters", async () => {
  const h = harness();
  const initial = h.model.load();
  h.calls[0].resolve(result);
  await initial;
  const filter = { ...h.view.filter, cmd: "test" as const, outcome: "failed" as const };
  const changed = h.model.filter(filter);
  h.calls[1].resolve({ ...result, filter });
  await changed;
  const day = h.model.selectDay("2026-03-30");
  assert.deepEqual(h.calls[2].args, { filter: { ...filter, day: "2026-03-30" } });
  h.calls[2].resolve({ ...result, filter: { ...filter, day: "2026-03-30" } });
  await day;
  // Opening and closing existing RunView does not mutate this controller.
  const refreshed = h.model.load();
  assert.deepEqual(h.calls[3].args, h.calls[2].args);
  h.calls[3].resolve({ ...result, filter: h.view.filter });
  await refreshed;
  const cleared = h.model.filter({ ...h.view.filter, cmd: "build" });
  assert.equal((h.calls[4].args!.filter as typeof filter & { day: string | null }).day, null);
  h.calls[4].resolve(result);
  await cleared;
});
test("late filters, errors and disposed workspace requests cannot replace the latest view", async () => {
  const h = harness();
  const old = h.model.load();
  const latest = h.model.filter({ ...result.filter, cmd: "test" });
  h.calls[1].resolve({ ...result, total: 9 });
  await latest;
  h.calls[0].reject(new Error("stale failure"));
  await old;
  assert.equal(h.view.data?.total, 9);
  assert.equal(h.view.error, null);
  const disposed = h.model.load();
  h.model.dispose();
  h.calls[2].resolve(result);
  await disposed;
  assert.equal(h.view.data, null);
});
test("errors remove stale data and retry preserves requested conditions", async () => {
  const h = harness();
  const failed = h.model.filter(result.filter);
  h.calls[0].reject(new Error("database busy"));
  await failed;
  assert.match(h.view.error!, /busy/);
  assert.equal(h.view.loading, false);
  const retry = h.model.load();
  assert.deepEqual(h.calls[1].args, h.calls[0].args);
  h.calls[1].resolve(result);
  await retry;
  assert.equal(h.view.error, null);
  assert.equal(h.view.data?.total, 2);
});
