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
  filter: { start: "2026-03-01", end: "2026-03-30", cmd: null, outcome: null },
  today: "2026-03-30",
  days: [{ day: "2026-03-30", count: 2 }],
};
test("default request uses core calendar and refresh retains all filters", async () => {
  const h = harness();
  const initial = h.model.load();
  assert.deepEqual(h.calls[0].args, {
    filter: { start: null, end: null, cmd: null, outcome: null },
  });
  h.calls[0].resolve(result);
  await initial;
  const filter = { ...h.view.filter, cmd: "test" as const, outcome: "failed" as const };
  const changed = h.model.load(filter);
  assert.deepEqual(h.calls[1].args, { filter });
  h.calls[1].resolve({ ...result, filter });
  await changed;
  const refreshed = h.model.load();
  assert.deepEqual(h.calls[2].args, { filter });
  h.calls[2].resolve({ ...result, filter });
  await refreshed;
  assert.deepEqual(h.view.filter, filter);
});
test("late filters, errors and disposed workspace requests cannot replace the latest view", async () => {
  const h = harness();
  const old = h.model.load();
  const latest = h.model.load({ ...result.filter, cmd: "test" });
  h.calls[1].resolve({ ...result, days: [{ day: "2026-03-30", count: 9 }] });
  await latest;
  h.calls[0].reject(new Error("stale failure"));
  await old;
  assert.equal(h.view.data?.days[0].count, 9);
  assert.equal(h.view.error, null);
  const disposed = h.model.load();
  h.model.dispose();
  h.calls[2].resolve(result);
  await disposed;
  assert.equal(h.view.data, null);
});
test("errors remove stale data and retry preserves requested conditions", async () => {
  const h = harness();
  const failed = h.model.load(result.filter);
  h.calls[0].reject(new Error("database busy"));
  await failed;
  assert.match(h.view.error!, /busy/);
  assert.equal(h.view.loading, false);
  const retry = h.model.load();
  assert.deepEqual(h.calls[1].args, h.calls[0].args);
  h.calls[1].resolve(result);
  await retry;
  assert.equal(h.view.error, null);
  assert.equal(h.view.data?.days[0].count, 2);
});
