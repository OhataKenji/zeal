import { test } from "node:test";
import assert from "node:assert/strict";
import {
  eventColor,
  layoutProfile,
  clampRange,
  zoomRange,
  panRange,
  scrollbarGeometry,
  scrollRange,
  timeTicks,
  timeToX,
  xToTime,
  visibleTracks,
  LANE_HEADER,
  formatRulerTime,
  formatCounter,
  counterPointY,
  eventPosition,
  COUNTER_HEIGHT,
} from "../src/lib/timeline.ts";
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

test("nested spans reuse the shallowest row and equal starts place the outer span first", () => {
  const events = [
    { ph: "X", ts: 0, dur: 100 },
    { ph: "X", ts: 10, dur: 20 },
    { ph: "X", ts: 12, dur: 8 },
    { ph: "X", ts: 35, dur: 5 },
    { ph: "X", ts: 45, dur: 10 },
    { ph: "X", ts: 45, dur: 30 },
    { ph: "X", ts: 60, dur: 30 },
  ];
  const lane = layoutProfile({ phases: [], lanes: [{ tid: 1, name: "worker", events }] }).lanes[0];
  assert.deepEqual(
    lane.events.map((event) => event.row),
    [0, 1, 2, 1, 2, 1, 2],
  );
  assert.deepEqual(
    lane.events.map((event) => event.event),
    events,
  );
  assert.equal(lane.spanRows, 3);
});

test("zoom keeps the pointer time fixed and pan respects the full negative-time domain", () => {
  const domain = { start: -100, end: 900 };
  const range = { start: 100, end: 500 };
  const zoomed = zoomRange(range, domain, 2, 0.25);
  assert.deepEqual(zoomed, { start: 150, end: 350 });
  assert.equal(zoomed.start + (zoomed.end - zoomed.start) * 0.25, 200);
  assert.deepEqual(panRange(zoomed, domain, -1000), { start: -100, end: 100 });
  assert.deepEqual(panRange(zoomed, domain, 1000), { start: 700, end: 900 });
  assert.deepEqual(zoomRange(range, domain, 0.01), domain);
  assert.equal(clampRange({ start: 250, end: 250 }, domain).end, 251);
});

test("one time transform aligns ruler, tracks and pointer coordinates after resize and pan", () => {
  for (const width of [900, 600, 0]) {
    const range = { start: -100, end: 900 };
    const px = timeToX(250, range, width, 190);
    assert.ok(Math.abs(xToTime(px, range, width, 190) - 250) < 1e-9);
    assert.equal(timeToX(range.start, range, width, 190), 190);
    assert.equal(timeToX(range.end, range, width, 190), 190 + Math.max(1, width - 190));
  }
  const shifted = panRange({ start: -100, end: 400 }, { start: -100, end: 900 }, 200);
  assert.equal(timeToX(350, shifted, 690, 190), 440);
});

test("ruler uses readable ticks across negative time, zoom and narrow viewports", () => {
  assert.deepEqual(
    timeTicks({ start: -100, end: 900 }, 900),
    [-100, 0, 100, 200, 300, 400, 500, 600, 700, 800, 900],
  );
  assert.deepEqual(timeTicks({ start: 120, end: 220 }, 450), [120, 140, 160, 180, 200, 220]);
  assert.deepEqual(timeTicks({ start: -100, end: 900 }, 0), [0]);
  const seconds = { start: -10, end: 5_000_000 };
  assert.equal(formatRulerTime(0, seconds), "0s");
  assert.equal(formatRulerTime(1_000_000, seconds), "1s");
  assert.equal(formatRulerTime(500, { start: 0, end: 10_000 }), "0.5ms");
  assert.equal(formatCounter(1.05500000000002), "1.055");
  assert.equal(formatCounter(0.000125555), "1.256e-4");
});

test("narrow zoom at large absolute timestamps generates bounded, distinct ruler ticks", () => {
  const range = { start: 1.8e15, end: 1.8e15 + 1 };
  const ticks = timeTicks(range, 900);
  assert.ok(ticks.length > 0 && ticks.length <= 12);
  assert.ok(
    ticks.every((time) => Number.isFinite(time) && time >= range.start && time <= range.end),
  );
  assert.ok(ticks.every((time, index) => index === 0 || time > ticks[index - 1]));
});

test("collapsing a thread only changes track geometry, preserving all threads and events", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      { tid: 1, name: "worker", events: [{ ph: "X", ts: 0, dur: 100 }] },
      { tid: 2, name: "worker", events: [{ ph: "i", ts: 20 }] },
    ],
  });
  const tracks = visibleTracks(layout.lanes, new Set([1]));
  assert.equal(tracks[0].height, LANE_HEADER);
  assert.equal(tracks[1].top, LANE_HEADER);
  assert.equal(tracks[1].height, layout.lanes[1].height);
  assert.equal(tracks[0].lane.events, layout.lanes[0].events);
  assert.deepEqual(
    tracks.map((track) => track.lane.tid),
    [1, 2],
  );
  assert.equal(visibleTracks(layout.lanes, new Set())[1].top, layout.lanes[0].height);
});

test("displayed threads follow numeric tid order regardless of names", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      { tid: 10, name: "Critical Path", events: [] },
      { tid: 2, name: "worker-10", events: [] },
      { tid: 1, name: "Main Thread", events: [] },
      { tid: 3, name: "Garbage Collector", events: [] },
    ],
  });
  const tracks = visibleTracks(layout.lanes, new Set());
  assert.deepEqual(
    tracks.map((track) => track.lane.tid),
    [1, 2, 3, 10],
  );
});

test("same-named displayed threads retain distinct tids and event references", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      { tid: 8, name: "worker", events: [{ ph: "i", ts: 10 }] },
      { tid: 1, name: "Main Thread", events: [] },
      { tid: 3, name: "worker", events: [{ ph: "i", ts: 20 }] },
    ],
  });
  const tracks = visibleTracks(layout.lanes, new Set());
  assert.deepEqual(
    tracks.map((track) => track.lane.tid),
    [1, 3, 8],
  );
  assert.equal(tracks[1].lane, layout.lanes[2]);
  assert.equal(tracks[2].lane, layout.lanes[0]);
});

test("display sorting does not mutate source lanes or raw event order", () => {
  const profile: Profile = {
    phases: [],
    lanes: [
      {
        tid: 2,
        name: "worker-2",
        events: [
          { ph: "i", ts: 20 },
          { ph: "i", ts: 10 },
        ],
      },
      { tid: 1, name: "worker-1", events: [] },
    ],
  };
  const original = JSON.stringify(profile);
  const layout = layoutProfile(profile);
  const lanes = [...layout.lanes];
  const tracks = visibleTracks(layout.lanes, new Set());
  assert.deepEqual(layout.lanes, lanes);
  assert.equal(JSON.stringify(profile), original);
  assert.equal(tracks[1].lane.events[0].event, profile.lanes[0].events[0]);
  assert.equal(tracks[1].lane.events[1].event, profile.lanes[0].events[1]);
});

test("displayed track positions follow sorted order and collapsed heights", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      { tid: 30, name: "Main Thread", events: [] },
      { tid: 10, name: "worker", events: [] },
      { tid: 20, name: "Critical Path", events: [] },
    ],
  });
  const tracks = visibleTracks(layout.lanes, new Set([10]));
  assert.deepEqual(
    tracks.map((track) => [track.lane.tid, track.top]),
    [
      [10, 0],
      [20, 20],
      [30, 66],
    ],
  );
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

test("positive resource counters use a zero baseline like BuildBuddy", () => {
  const lane = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 1,
        name: "main",
        events: [
          { ph: "C", ts: 0, name: "Memory usage (Bazel)", args: { memory: 100 } },
          { ph: "C", ts: 1, name: "Memory usage (Bazel)", args: { memory: 200 } },
        ],
      },
    ],
  }).lanes[0];
  const counter = lane.counters[0];
  assert.deepEqual([counter.min, counter.max], [0, 200]);
  assert.equal(counterPointY(counter, 100), 22);
  assert.equal(counterPointY(counter, 200), 7);
  assert.deepEqual(
    counter.points.map((point) => point.value),
    [100, 200],
  );
});

test("constant positive counters plot above zero instead of on the baseline", () => {
  const counter = layoutProfile({
    phases: [],
    lanes: [{ tid: 1, name: "", events: [{ ph: "C", ts: 0, name: "cpu", args: { cpu: 2 } }] }],
  }).lanes[0].counters[0];
  assert.deepEqual([counter.min, counter.max], [0, 2]);
  assert.equal(counterPointY(counter, 2), 7);
});

test("zero and negative counters retain finite geometry and their original values", () => {
  const lane = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 1,
        name: "",
        events: [
          { ph: "C", ts: 0, name: "zero", args: { value: 0 } },
          { ph: "C", ts: 1, name: "negative", args: { value: -2 } },
        ],
      },
    ],
  }).lanes[0];
  const [zero, negative] = lane.counters;
  assert.deepEqual([zero.min, zero.max], [0, 0]);
  assert.equal(counterPointY(zero, 0), 37);
  assert.deepEqual([negative.min, negative.max], [-2, 0]);
  assert.equal(counterPointY(negative, -2), 37);
  assert.equal(counterPointY(negative, 0), 7);
  assert.equal(negative.points[0].value, -2);
});

test("all counter series follow every event track without changing raw geometry", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 1,
        name: "main",
        events: [{ ph: "C", ts: 10, name: "cpu", args: { system: 1, user: 2 } }],
      },
      { tid: 2, name: "worker", events: [{ ph: "X", ts: 0, dur: 20 }] },
    ],
  });
  const tracks = visibleTracks(layout.lanes, new Set());
  assert.deepEqual(
    tracks.map((track) => [track.kind, track.lane.tid, track.top, track.height]),
    [
      ["events", 1, 0, 46],
      ["events", 2, 46, 46],
      ["counter", 1, 92, COUNTER_HEIGHT],
      ["counter", 1, 134, COUNTER_HEIGHT],
    ],
  );
  assert.equal(tracks[2].lane.events, layout.lanes[0].events);
});

test("collapsing threads keeps every counter series visible", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      { tid: 1, name: "main", events: [{ ph: "C", ts: 10, name: "cpu", args: { used: 2 } }] },
    ],
  });
  const tracks = visibleTracks(layout.lanes, new Set([1]));
  assert.deepEqual(
    tracks.map((track) => [track.kind, track.top, track.height]),
    [
      ["events", 0, LANE_HEADER],
      ["counter", LANE_HEADER, COUNTER_HEIGHT],
    ],
  );
});

test("known Bazel counters follow resource order across source threads", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 2,
        name: "main",
        events: [
          { ph: "C", ts: 1, name: "Network Down usage (total)", args: { value: 1 } },
          { ph: "C", ts: 1, name: "Memory usage (total)", args: { value: 1 } },
          { ph: "C", ts: 1, name: "Memory usage (Bazel)", args: { value: 1 } },
          { ph: "C", ts: 1, name: "action count (local)", args: { value: 1 } },
        ],
      },
      {
        tid: 1,
        name: "metrics",
        events: [
          { ph: "C", ts: 1, name: "Network Up usage (total)", args: { value: 1 } },
          { ph: "C", ts: 1, name: "System load average", args: { value: 1 } },
          { ph: "C", ts: 1, name: "CPU usage (total)", args: { value: 1 } },
          { ph: "C", ts: 1, name: "CPU usage (Bazel)", args: { value: 1 } },
          { ph: "C", ts: 1, name: "action count", args: { "local action cache": 2, action: 1 } },
        ],
      },
    ],
  });
  const counters = visibleTracks(layout.lanes, new Set()).filter(
    (track) => track.kind === "counter",
  );
  assert.deepEqual(
    counters.map((track) => {
      const counter = track.counter;
      return [counter.name, counter.field];
    }),
    [
      ["action count", "action"],
      ["action count", "local action cache"],
      ["action count (local)", "value"],
      ["CPU usage (Bazel)", "value"],
      ["Memory usage (Bazel)", "value"],
      ["CPU usage (total)", "value"],
      ["Memory usage (total)", "value"],
      ["System load average", "value"],
      ["Network Up usage (total)", "value"],
      ["Network Down usage (total)", "value"],
    ],
  );
});

test("unknown counter names and fields sort after known metrics without merging tids", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 2,
        name: "same",
        events: [
          { ph: "C", ts: 1, name: "zeta", args: { value: 1 } },
          { ph: "C", ts: 1, name: "alpha", args: { z: 2, a: 1 } },
        ],
      },
      {
        tid: 1,
        name: "same",
        events: [
          { ph: "C", ts: 1, name: "alpha", args: { a: 3 } },
          { ph: "C", ts: 1, name: "CPU usage (Bazel)", args: { value: 2 } },
        ],
      },
    ],
  });
  const counters = visibleTracks(layout.lanes, new Set()).filter(
    (track) => track.kind === "counter",
  );
  assert.deepEqual(
    counters.map((track) => {
      const counter = track.counter;
      return [counter.name, counter.field, track.lane.tid];
    }),
    [
      ["CPU usage (Bazel)", "value", 1],
      ["alpha", "a", 1],
      ["alpha", "a", 2],
      ["alpha", "z", 2],
      ["zeta", "value", 2],
    ],
  );
});

test("revealing a numeric counter resolves its displayed series and original event", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 1,
        name: "main",
        events: [
          { ph: "X", ts: 0, dur: 10 },
          { ph: "C", ts: 10, name: "cpu", args: { z: 5, a: 2 } },
        ],
      },
    ],
  });
  const tracks = visibleTracks(layout.lanes, new Set([1]));
  const position = eventPosition(tracks, 1, 1)!;
  const counter = layout.lanes[0].counters[1];
  assert.equal(position.track, tracks[1]);
  assert.equal(position.y, tracks[1].top + counterPointY(counter, 2));
  assert.equal(position.track.lane.events[1].index, 1);
});

test("raw and untimed counters resolve to their original event track", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 1,
        name: "main",
        events: [
          { ph: "C", ts: 1, name: "raw", args: { state: "busy" } },
          { ph: "C", name: "untimed", args: { value: 2 } },
        ],
      },
    ],
  });
  const tracks = visibleTracks(layout.lanes, new Set());
  assert.deepEqual(eventPosition(tracks, 1, 0), { track: tracks[0], y: 29 });
  assert.deepEqual(eventPosition(tracks, 1, 1), { track: tracks[0], y: 29 });
});

test("event colors use cname before name and stay stable", () => {
  assert.equal(
    eventColor({ name: "one", cname: "good" }),
    eventColor({ name: "two", cname: "good" }),
  );
  assert.equal(eventColor({ name: "stable" }), eventColor({ name: "stable" }));
  assert.notEqual(eventColor({ name: "one" }), eventColor({ name: "two" }));
  assert.equal(eventColor({ cname: "custom" }), eventColor({ name: "custom" }));
  assert.equal(eventColor({}), eventColor({ name: "(unnamed event)" }));
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

test("a fitted timeline has no horizontal scroll travel", () => {
  const domain = { start: -200, end: 800 };
  assert.deepEqual(scrollbarGeometry(domain, domain, 500), { width: 500, left: 0 });
  assert.deepEqual(scrollRange(domain, domain, 0.75), domain);
});

test("horizontal scrolling reaches both ends without changing the zoom", () => {
  const domain = { start: -200, end: 800 };
  const range = { start: 100, end: 300 };
  assert.deepEqual(scrollbarGeometry(range, domain, 500), { width: 2500, left: 750 });
  assert.deepEqual(scrollRange(range, domain, 0), { start: -200, end: 0 });
  assert.deepEqual(scrollRange(range, domain, 0.5), { start: 200, end: 400 });
  assert.deepEqual(scrollRange(range, domain, 1), { start: 600, end: 800 });
  assert.deepEqual(scrollRange(range, domain, -1), { start: -200, end: 0 });
  assert.deepEqual(scrollRange(range, domain, 2), { start: 600, end: 800 });
});

test("scrollbar position follows pinch zoom, dragging and viewport resizing", () => {
  const domain = { start: 0, end: 1000 };
  const zoomed = zoomRange(domain, domain, 2);
  const panned = panRange(zoomed, domain, 125);
  const geometry = scrollbarGeometry(panned, domain, 800);
  assert.deepEqual(geometry, { width: 1600, left: 600 });
  assert.deepEqual(scrollRange(panned, domain, geometry.left / (geometry.width - 800)), panned);
  assert.deepEqual(scrollbarGeometry(panned, domain, 400), { width: 800, left: 300 });
});

test("deep zoom keeps native scroll geometry bounded and the trace end reachable", () => {
  const domain = { start: 0, end: 1_000_000_000 };
  const range = { start: 999_999_999, end: 1_000_000_000 };
  assert.deepEqual(scrollbarGeometry(range, domain, 500), { width: 1_000_000, left: 999_500 });
  assert.deepEqual(scrollRange(range, domain, 1), range);
});
