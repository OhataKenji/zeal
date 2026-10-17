import { test } from "node:test";
import assert from "node:assert/strict";
import { drawRuler, drawTimeline } from "../src/lib/timeline-canvas.ts";
import { layoutProfile, visibleTracks } from "../src/lib/timeline.ts";
import type { Profile } from "../src/lib/types.ts";

// This boundary supports only the canvas operations the renderer uses. Unknown
// methods fail instead of silently succeeding, as they would with a Proxy.
function recordingCanvas() {
  const outlines: number[][] = [];
  const fills: { bounds: number[]; clips: number[][] }[] = [];
  const strokedLines: number[][] = [];
  const strokes: { lines: number[][]; clips: number[][] }[] = [];
  let path: number[][] = [];
  let rectangle: number[] | undefined;
  let clips: number[][] = [];
  const savedClips: number[][][] = [];
  const strokedArcs: { x: number; y: number; radius: number }[] = [];
  let pathArcs: typeof strokedArcs = [];
  const lines: number[][] = [];
  let position = [0, 0];
  const canvas = {
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 1,
    save() {
      savedClips.push(clips);
    },
    restore() {
      clips = savedClips.pop()!;
    },
    beginPath() {
      path = [];
      pathArcs = [];
      rectangle = undefined;
    },
    closePath() {},
    rect(x: number, y: number, width: number, height: number) {
      rectangle = [x, y, width, height];
    },
    clip() {
      assert.ok(rectangle, "the renderer clips with a rectangular path");
      clips = [...clips, rectangle];
    },
    fillText() {},
    fillRect(x: number, y: number, width: number, height: number) {
      fills.push({ bounds: [x, y, width, height], clips });
    },
    fill() {},
    stroke() {
      strokes.push({ lines: path, clips });
      strokedLines.push(...path);
      strokedArcs.push(...pathArcs);
    },
    moveTo(x: number, y: number) {
      position = [x, y];
    },
    lineTo(x: number, y: number) {
      const line = [...position, x, y];
      lines.push(line);
      path.push(line);
      position = [x, y];
    },
    strokeRect(x: number, y: number, width: number, height: number) {
      outlines.push([x, y, width, height]);
    },
    arc(x: number, y: number, radius: number) {
      pathArcs.push({ x, y, radius });
    },
  };
  return {
    context: canvas as unknown as CanvasRenderingContext2D,
    outlines,
    strokedArcs,
    lines,
    fills,
    strokedLines,
    strokes,
  };
}

type Frame = Parameters<typeof drawTimeline>[1];

function render(
  lanes: Profile["lanes"],
  options: Partial<Omit<Frame, "tracks">> & {
    collapsed?: number[];
  } = {},
) {
  const { collapsed = [], ...viewport } = options;
  const layout = layoutProfile({ phases: [], lanes });
  const canvas = recordingCanvas();
  const hits = drawTimeline(canvas.context, {
    width: 200,
    height: 200,
    gutter: 100,
    scrollTop: 0,
    range: { start: 0, end: 100 },
    ticks: [],
    selection: null,
    ...viewport,
    tracks: visibleTracks(layout.lanes, new Set(collapsed)),
  });
  return { ...canvas, hits };
}

test("a span crossing the visible time range keeps its clipped hit area", () => {
  const { hits } = render([{ tid: 7, name: "worker", events: [{ ph: "X", ts: 0, dur: 100 }] }], {
    width: 160,
    range: { start: 20, end: 80 },
  });

  assert.deepEqual(hits, [{ x: 98, y: 20, width: 60, height: 18, tid: 7, index: 0 }]);
});

test("overlapping spans are selectable on separate rows", () => {
  const { hits } = render([
    {
      tid: 7,
      name: "worker",
      events: [
        { ph: "X", ts: 0, dur: 100 },
        { ph: "X", ts: 40, dur: 20 },
      ],
    },
  ]);

  assert.deepEqual(hits, [
    { x: 98, y: 20, width: 100, height: 18, tid: 7, index: 0 },
    { x: 138, y: 38, width: 20, height: 18, tid: 7, index: 1 },
  ]);
});

test("an instant event retains a usable hit area", () => {
  const { hits } = render([{ tid: 7, name: "worker", events: [{ ph: "i", ts: 70 }] }]);

  assert.deepEqual(hits, [{ x: 168, y: 20, width: 6, height: 18, tid: 7, index: 0 }]);
});

test("events without a visible timestamp do not expose hit areas", () => {
  const { hits } = render([
    {
      tid: 7,
      name: "worker",
      events: [{ name: "untimed" }, { ph: "X", ts: -20, dur: 10 }, { ph: "i", ts: 110 }],
    },
  ]);

  assert.deepEqual(hits, []);
});

test("the selected span outline identifies its thread as well as its event index", () => {
  const { outlines } = render(
    [
      { tid: 7, name: "worker", events: [{ ph: "X", ts: 10, dur: 20 }] },
      { tid: 8, name: "worker", events: [{ ph: "X", ts: 40, dur: 20 }] },
    ],
    { selection: { tid: 8, index: 0 } },
  );

  assert.deepEqual(outlines, [[140, 66.5, 20, 16]]);
});

test("scrolling excludes lanes outside the viewport and offsets the visible hit", () => {
  const { hits } = render(
    [
      { tid: 7, name: "worker-1", events: [{ ph: "X", ts: 10, dur: 20 }] },
      { tid: 8, name: "worker-2", events: [{ ph: "X", ts: 10, dur: 20 }] },
      { tid: 9, name: "worker-3", events: [{ ph: "X", ts: 10, dur: 20 }] },
    ],
    { scrollTop: 50, height: 40 },
  );

  assert.deepEqual(hits, [{ x: 108, y: 16, width: 20, height: 18, tid: 8, index: 0 }]);
});

test("collapsing a lane removes its hits and moves the next lane upward", () => {
  const { hits } = render(
    [
      { tid: 7, name: "closed", events: [{ ph: "X", ts: 10, dur: 20 }] },
      { tid: 8, name: "open", events: [{ ph: "X", ts: 10, dur: 20 }] },
    ],
    { collapsed: [7] },
  );

  assert.deepEqual(hits, [{ x: 108, y: 40, width: 20, height: 18, tid: 8, index: 0 }]);
});

test("equal thread names retain distinct event identities", () => {
  const { hits } = render([
    { tid: 8, name: "worker", events: [{ ph: "i", ts: 10 }] },
    { tid: 7, name: "worker", events: [{ ph: "i", ts: 10 }] },
  ]);

  assert.deepEqual(
    hits.map(({ tid, index }) => ({ tid, index })),
    [
      { tid: 7, index: 0 },
      { tid: 8, index: 0 },
    ],
  );
});

test("numeric counters are visible initially without duplicate span-row marks", () => {
  const { hits } = render([
    { tid: 7, name: "metrics", events: [{ ph: "C", ts: 10, name: "cpu", args: { usage: 1 } }] },
  ]);

  assert.deepEqual(hits, [{ x: 106, y: 79, width: 8, height: 8, tid: 7, index: 0 }]);
});

test("a counter without numeric fields remains selectable as a raw mark", () => {
  const { hits } = render([
    {
      tid: 7,
      name: "metrics",
      events: [{ ph: "C", ts: 30, name: "status", args: { text: "busy" } }],
    },
  ]);

  assert.deepEqual(hits, [{ x: 128, y: 20, width: 6, height: 18, tid: 7, index: 0 }]);
});

test("counter tracks expose hits only for points within the visible time range", () => {
  const { hits } = render([
    {
      tid: 7,
      name: "metrics",
      events: [
        { ph: "C", ts: -10, name: "cpu", args: { usage: 1 } },
        { ph: "C", ts: 50, name: "cpu", args: { usage: 2 } },
        { ph: "C", ts: 110, name: "cpu", args: { usage: 3 } },
      ],
    },
  ]);

  assert.deepEqual(hits, [{ x: 146, y: 64, width: 8, height: 8, tid: 7, index: 1 }]);
});

test("the selected counter point receives a ring at its hit center", () => {
  const { hits, strokedArcs } = render(
    [
      {
        tid: 7,
        name: "metrics",
        events: [
          { ph: "C", ts: 10, name: "cpu", args: { usage: 1 } },
          { ph: "C", ts: 90, name: "cpu", args: { usage: 3 } },
        ],
      },
    ],
    { selection: { tid: 7, index: 1 } },
  );

  assert.deepEqual(
    strokedArcs.filter(({ radius }) => radius === 4),
    [{ x: hits[1].x + hits[1].width / 2, y: hits[1].y + hits[1].height / 2, radius: 4 }],
  );
});

test("ruler ticks and timeline grid lines share the same horizontal positions", () => {
  const axis = { width: 220, gutter: 100, range: { start: 20, end: 80 }, ticks: [20, 50, 80] };
  const ruler = recordingCanvas();
  drawRuler(ruler.context, axis);
  const chart = render([], { ...axis, height: 100 });

  assert.deepEqual(
    ruler.lines.map(([x]) => x),
    [100.5, 160.5, 220.5],
  );
  assert.deepEqual(
    chart.lines.filter(([x]) => x >= axis.gutter).map(([x]) => x),
    ruler.lines.map(([x]) => x),
  );
});

test("a span paints its body at the selectable event position", () => {
  const { hits, fills } = render([
    { tid: 7, name: "worker", events: [{ ph: "X", ts: 10, dur: 20 }] },
  ]);

  assert.deepEqual(hits, [{ x: 108, y: 20, width: 20, height: 18, tid: 7, index: 0 }]);
  assert.deepEqual(fills.find(({ bounds }) => bounds[0] === 110)?.bounds, [110, 20, 20, 17]);
});

test("a span is painted inside the track clip after the gutter and lane header", () => {
  const { fills } = render([{ tid: 7, name: "worker", events: [{ ph: "X", ts: 10, dur: 20 }] }]);

  assert.deepEqual(fills.find(({ bounds }) => bounds[0] === 110)?.clips, [[100, 20, 100, 26]]);
});

test("a counter track paints a connecting line between sample positions", () => {
  const { strokedLines } = render([
    {
      tid: 7,
      name: "metrics",
      events: [
        { ph: "C", ts: 10, name: "cpu", args: { usage: 1 } },
        { ph: "C", ts: 90, name: "cpu", args: { usage: 3 } },
      ],
    },
  ]);

  assert.deepEqual(
    strokedLines.find(([start, , end]) => start === 110 && end === 190),
    [110, 83, 190, 53],
  );
});

test("mixed events keep their original identities when counters move below the threads", () => {
  const { hits } = render([
    {
      tid: 7,
      name: "worker",
      events: [
        { ph: "X", ts: 10, dur: 20 },
        { ph: "i", ts: 30 },
        { ph: "C", ts: 50, name: "cpu", args: { usage: 1 } },
        { ph: "C", ts: 70, name: "status", args: { text: "busy" } },
        { ph: "C", name: "untimed", args: { usage: 2 } },
      ],
    },
    { tid: 8, name: "another worker", events: [{ ph: "i", ts: 90 }] },
  ]);

  assert.deepEqual(
    hits.map(({ tid, index }) => ({ tid, index })),
    [
      { tid: 7, index: 0 },
      { tid: 7, index: 1 },
      { tid: 7, index: 3 },
      { tid: 8, index: 0 },
      { tid: 7, index: 2 },
    ],
  );
});

test("collapsing a thread keeps its separate counter visible at the new position", () => {
  const { hits } = render(
    [
      {
        tid: 7,
        name: "worker",
        events: [
          { ph: "X", ts: 10, dur: 20 },
          { ph: "C", ts: 50, name: "cpu", args: { usage: 1 } },
        ],
      },
    ],
    { collapsed: [7] },
  );

  assert.deepEqual(hits, [{ x: 146, y: 53, width: 8, height: 8, tid: 7, index: 1 }]);
});

test("counter tracks follow global series order rather than their thread order", () => {
  const { hits } = render([
    { tid: 7, name: "worker", events: [{ ph: "C", ts: 50, name: "z metric", args: { value: 1 } }] },
    { tid: 8, name: "worker", events: [{ ph: "C", ts: 50, name: "a metric", args: { value: 1 } }] },
  ]);

  assert.deepEqual(hits, [
    { x: 146, y: 125, width: 8, height: 8, tid: 8, index: 0 },
    { x: 146, y: 167, width: 8, height: 8, tid: 7, index: 0 },
  ]);
});

test("equal counter names from different threads remain separate rows", () => {
  const { hits } = render([
    { tid: 7, name: "worker", events: [{ ph: "C", ts: 50, name: "cpu", args: { usage: 1 } }] },
    { tid: 8, name: "worker", events: [{ ph: "C", ts: 50, name: "cpu", args: { usage: 3 } }] },
  ]);

  assert.deepEqual(hits, [
    { x: 146, y: 125, width: 8, height: 8, tid: 7, index: 0 },
    { x: 146, y: 167, width: 8, height: 8, tid: 8, index: 0 },
  ]);
});

test("scrolling to the counters excludes rows outside the viewport", () => {
  const { hits } = render(
    [
      {
        tid: 7,
        name: "metrics",
        events: [
          { ph: "C", ts: 50, name: "a metric", args: { value: 1 } },
          { ph: "C", ts: 50, name: "b metric", args: { value: 1 } },
          { ph: "C", ts: 50, name: "c metric", args: { value: 1 } },
        ],
      },
    ],
    { scrollTop: 96, height: 30 },
  );

  assert.deepEqual(hits, [{ x: 146, y: 25, width: 8, height: 8, tid: 7, index: 1 }]);
});

test("a counter line is painted inside its own row clip", () => {
  const { strokes } = render([
    {
      tid: 7,
      name: "metrics",
      events: [
        { ph: "C", ts: 10, name: "cpu", args: { usage: 1 } },
        { ph: "C", ts: 90, name: "cpu", args: { usage: 3 } },
      ],
    },
  ]);

  assert.deepEqual(strokes.find(({ lines }) => lines.some(([x]) => x === 110))?.clips, [
    [100, 46, 100, 42],
  ]);
});
