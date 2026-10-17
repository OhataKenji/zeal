import { test } from "node:test";
import assert from "node:assert/strict";
import { drawTimeline } from "../src/lib/timeline-canvas.ts";
import { layoutProfile, visibleTracks } from "../src/lib/timeline.ts";

// Record the commands relevant to selection geometry without a browser or pixels.
function recordingContext() {
  const commands: unknown[][] = [];
  const context = new Proxy(
    {},
    {
      get(target, key) {
        return Reflect.get(target, key) ?? ((...args: unknown[]) => commands.push([key, ...args]));
      },
    },
  ) as CanvasRenderingContext2D;
  return { context, commands };
}

test("clipped spans and marks retain their clickable rows and selected outline", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 7,
        name: "worker",
        events: [
          { ph: "X", ts: 0, dur: 100 },
          { ph: "X", ts: 40, dur: 20 },
          { ph: "i", ts: 70 },
          { name: "untimed" },
        ],
      },
    ],
  });
  const { context, commands } = recordingContext();
  const hits = drawTimeline(context, {
    width: 250,
    height: 100,
    gutter: 190,
    scrollTop: 0,
    range: { start: 20, end: 80 },
    ticks: [],
    tracks: visibleTracks(layout.lanes, new Set()),
    selection: { tid: 7, index: 1 },
  });
  assert.deepEqual(hits, [
    { x: 188, y: 20, width: 60, height: 18, tid: 7, index: 0 },
    { x: 208, y: 38, width: 20, height: 18, tid: 7, index: 1 },
    { x: 238, y: 20, width: 6, height: 18, tid: 7, index: 2 },
  ]);
  assert.deepEqual(
    commands.filter(([name]) => name === "strokeRect"),
    [["strokeRect", 210, 38.5, 20, 16]],
  );
});

test("counter folding exposes only visible series points and keeps raw counter marks", () => {
  const layout = layoutProfile({
    phases: [],
    lanes: [
      {
        tid: 7,
        name: "metrics",
        events: [
          { ph: "C", ts: 10, name: "cpu", args: { usage: 1 } },
          { ph: "C", ts: 30, name: "status", args: { text: "busy" } },
          { ph: "C", ts: 90, name: "cpu", args: { usage: 3 } },
        ],
      },
    ],
  });
  const frame = {
    width: 290,
    height: 120,
    gutter: 190,
    scrollTop: 0,
    range: { start: 0, end: 100 },
    ticks: [],
    selection: { tid: 7, index: 2 },
  };
  const folded = drawTimeline(recordingContext().context, {
    ...frame,
    tracks: visibleTracks(layout.lanes, new Set()),
  });
  assert.deepEqual(folded, [{ x: 218, y: 20, width: 6, height: 18, tid: 7, index: 1 }]);
  const { context, commands } = recordingContext();
  const expanded = drawTimeline(context, {
    ...frame,
    tracks: visibleTracks(layout.lanes, new Set(), new Set([7])),
  });
  assert.deepEqual(expanded, [
    ...folded,
    { x: 196, y: 91, width: 8, height: 8, tid: 7, index: 0 },
    { x: 276, y: 61, width: 8, height: 8, tid: 7, index: 2 },
  ]);
  assert.deepEqual(
    commands.filter(([name, , , radius]) => name === "arc" && radius === 4),
    [["arc", 280, 65, 4, 0, Math.PI * 2]],
  );
  assert.deepEqual(
    drawTimeline(recordingContext().context, {
      ...frame,
      tracks: visibleTracks(layout.lanes, new Set([7]), new Set([7])),
    }),
    [],
  );
});
