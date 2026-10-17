import { test } from "node:test";
import assert from "node:assert/strict";
import { eventColor, layoutProfile } from "../src/lib/timeline.ts";
import type { Profile } from "../src/lib/types.ts";

test("same-named threads stay separate and raw fields/file order survive layout", () => {
  const profile: Profile = {
    phases: [{ name: "Load, analyze dependencies and build artifacts", startUs: -10, endUs: 80 }],
    lanes: [
      {
        tid: 1,
        name: "worker",
        events: [
          { ph: "X", ts: 10, dur: 20, args: { target: "//:app" }, custom: true },
          { ph: "i", ts: -20 },
        ],
      },
      { tid: 2, name: "worker", events: [{ ph: "X", ts: 1, dur: 4 }] },
    ],
  };
  const before = JSON.stringify(profile);
  const result = layoutProfile(profile);
  assert.deepEqual(
    result.lanes.map((lane) => lane.tid),
    [1, 2],
  );
  assert.equal(result.start, -20);
  assert.equal(result.end, 80);
  assert.equal(result.lanes[0].events[0].event, profile.lanes[0].events[0]);
  assert.equal(result.lanes[0].events[1].event, profile.lanes[0].events[1]);
  assert.equal(JSON.stringify(profile), before);
});

test("overlapping spans stack and reuse rows only after the previous span ends", () => {
  const result = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 1,
        name: "",
        events: [
          { ph: "X", ts: 4, dur: 4 },
          { ph: "X", ts: 0, dur: 10 },
          { ph: "X", ts: 1, dur: 2 },
          { ph: "X", ts: 8, dur: 1 },
        ],
      },
    ],
  });
  assert.deepEqual(
    result.lanes[0].events.map((event) => event.row),
    [1, 0, 1, 1],
  );
  assert.equal(result.lanes[0].spanRows, 2);
});

test("instant and unknown events are marks and missing timestamps remain inspectable", () => {
  const result = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 1,
        name: "",
        events: [
          { ph: "i", ts: 1 },
          { ph: "s", ts: 2, id: "flow" },
          { name: "untimed", custom: [1, 2] },
        ],
      },
    ],
  });
  assert.deepEqual(
    result.lanes[0].events.map((event) => event.kind),
    ["mark", "mark", "mark"],
  );
  assert.equal(result.lanes[0].events[2].start, null);
  assert.deepEqual(result.lanes[0].events[2].event.custom, [1, 2]);
});

test("counter names and numeric fields form separate series without losing raw args", () => {
  const result = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 1,
        name: "metrics",
        events: [
          { ph: "C", ts: 0, name: "memory", args: { used: 10, free: 20, unit: "MB" } },
          { ph: "C", ts: 1, name: "memory", args: { used: 14, free: 16 } },
          { ph: "C", ts: 1, name: "cpu", args: { used: 2 } },
        ],
      },
    ],
  });
  assert.deepEqual(
    result.lanes[0].counters.map((series) => [series.name, series.field]),
    [
      ["memory", "used"],
      ["memory", "free"],
      ["cpu", "used"],
    ],
  );
  assert.deepEqual(
    result.lanes[0].counters[0].points.map((point) => point.value),
    [10, 14],
  );
  assert.deepEqual(result.lanes[0].events[0].event.args, { used: 10, free: 20, unit: "MB" });
});

test("cname takes precedence over deterministic name color", () => {
  assert.equal(
    eventColor({ name: "one", cname: "good" }),
    eventColor({ name: "two", cname: "good" }),
  );
  assert.equal(eventColor({ name: "stable" }), eventColor({ name: "stable" }));
  assert.notEqual(eventColor({ name: "one" }), eventColor({ name: "two" }));
});

test("large trace retains all threads and events without a rendering limit", () => {
  const profile: Profile = {
    phases: [],
    lanes: Array.from({ length: 100 }, (_, tid) => ({
      tid,
      name: "worker",
      events: Array.from({ length: 500 }, (_, ts) => ({ ph: "X", ts, dur: 2 })),
    })),
  };
  const result = layoutProfile(profile);
  assert.equal(result.lanes.length, 100);
  assert.equal(
    result.lanes.reduce((count, lane) => count + lane.events.length, 0),
    50_000,
  );
  assert.equal(result.lanes[99].events[499].event, profile.lanes[99].events[499]);
});
