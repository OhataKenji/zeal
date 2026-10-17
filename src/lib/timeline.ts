import type { Profile, TraceEvent } from "./types.ts";

export const ROW_HEIGHT = 22;
export const LANE_HEADER = 24;
export const COUNTER_HEIGHT = 64;

export interface EventGeometry {
  index: number;
  event: TraceEvent;
  start: number | null;
  end: number | null;
  row: number;
  kind: "span" | "mark" | "counter";
}

export interface CounterSeries {
  name: string;
  field: string;
  points: { time: number; value: number; index: number }[];
  min: number;
  max: number;
}

export interface LaneGeometry {
  tid: number;
  name: string;
  top: number;
  height: number;
  spanRows: number;
  events: EventGeometry[];
  counters: CounterSeries[];
}

export interface Timeline {
  start: number;
  end: number;
  height: number;
  lanes: LaneGeometry[];
}

export function numeric(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

export function eventName(event: TraceEvent): string {
  return typeof event.name === "string" ? event.name : "(unnamed event)";
}

// Chrome trace reserved cname values are protocol colors, not CSS names.
// Source: catapult tracing/tracing/base/color_scheme.html (reservedColorsByName).
// https://github.com/catapult-project/catapult/blob/main/tracing/tracing/base/color_scheme.html
const traceColors: Record<string, readonly [number, number, number]> = {
  thread_state_uninterruptible: [182, 125, 143],
  thread_state_iowait: [255, 140, 0],
  thread_state_running: [126, 200, 148],
  thread_state_runnable: [133, 160, 210],
  thread_state_sleeping: [240, 240, 240],
  thread_state_unknown: [199, 155, 125],
  background_memory_dump: [0, 180, 180],
  light_memory_dump: [0, 0, 180],
  detailed_memory_dump: [180, 0, 180],
  vsync_highlight_color: [0, 0, 255],
  generic_work: [125, 125, 125],
  good: [0, 125, 0],
  bad: [180, 125, 0],
  terrible: [180, 0, 0],
  black: [0, 0, 0],
  grey: [221, 221, 221],
  white: [255, 255, 255],
  yellow: [255, 255, 0],
  olive: [100, 100, 0],
  rail_response: [67, 135, 253],
  rail_animation: [244, 74, 63],
  rail_idle: [238, 142, 0],
  rail_load: [13, 168, 97],
  startup: [230, 230, 0],
  heap_dump_stack_frame: [128, 128, 128],
  heap_dump_object_type: [0, 0, 255],
  heap_dump_child_node_arrow: [204, 102, 0],
  cq_build_running: [255, 255, 119],
  cq_build_passed: [153, 238, 102],
  cq_build_failed: [238, 136, 136],
  cq_build_abandoned: [187, 187, 187],
  cq_build_attempt_runnig: [222, 222, 75],
  cq_build_attempt_passed: [103, 218, 35],
  cq_build_attempt_failed: [197, 81, 81],
};

function namedColor(event: TraceEvent): readonly [number, number, number] | undefined {
  return typeof event.cname === "string" && Object.hasOwn(traceColors, event.cname)
    ? traceColors[event.cname]
    : undefined;
}

export function eventColor(event: TraceEvent): string {
  const color = namedColor(event);
  if (color) return `rgb(${color.join(",")})`;
  const key = eventName(event);
  let hash = 2166136261;
  for (const char of key) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
  return `hsl(${(hash >>> 0) % 360} 42% 72%)`;
}

export function eventTextColor(event: TraceEvent): string {
  const color = namedColor(event);
  if (!color) return "#111";
  const [r, g, b] = color.map((value) => {
    const channel = value / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.179 ? "#fff" : "#111";
}

// Heap of the earliest-ending span in each row. Sorting only the geometry
// assigns overlap rows in O(n log n); lane.events stays in file order.
function stackSpans(events: EventGeometry[]): number {
  const heap: { end: number; row: number }[] = [];
  let rows = 0;
  for (const span of events
    .filter((e) => e.kind === "span" && e.start !== null)
    .sort((a, b) => a.start! - b.start! || a.index - b.index)) {
    let row: number;
    if (heap.length && heap[0].end <= span.start!) {
      row = heap[0].row;
      heap[0] = heap[heap.length - 1];
      heap.pop();
      let i = 0;
      while (2 * i + 1 < heap.length) {
        let child = 2 * i + 1;
        if (child + 1 < heap.length && heap[child + 1].end < heap[child].end) ++child;
        if (heap[i].end <= heap[child].end) break;
        [heap[i], heap[child]] = [heap[child], heap[i]];
        i = child;
      }
    } else row = rows++;
    span.row = row;
    heap.push({ end: span.end!, row });
    let i = heap.length - 1;
    while (i > 0) {
      const parent = Math.floor((i - 1) / 2);
      if (heap[parent].end <= heap[i].end) break;
      [heap[parent], heap[i]] = [heap[i], heap[parent]];
      i = parent;
    }
  }
  return Math.max(rows, 1);
}

export function layoutProfile(profile: Profile): Timeline {
  let start = Infinity;
  let end = -Infinity;
  let top = 0;
  const extend = (value: number) => {
    start = Math.min(start, value);
    end = Math.max(end, value);
  };
  profile.phases.forEach((phase) => {
    extend(phase.startUs);
    extend(phase.endUs);
  });
  const lanes = profile.lanes.map((lane) => {
    const series = new Map<string, CounterSeries>();
    const events: EventGeometry[] = lane.events.map((event, index) => {
      const time = numeric(event.ts);
      const duration = numeric(event.dur);
      const kind =
        event.ph === "C"
          ? "counter"
          : event.ph === "X" && duration !== null && duration >= 0
            ? "span"
            : "mark";
      const finish = time === null ? null : time + (kind === "span" ? duration! : 0);
      if (time !== null) extend(time);
      if (finish !== null) extend(finish);
      if (kind === "counter" && time !== null && event.args && typeof event.args === "object") {
        for (const [field, raw] of Object.entries(event.args)) {
          const value = numeric(raw);
          if (value === null) continue;
          const name = eventName(event);
          const key = JSON.stringify([name, field]);
          const counter = series.get(key) ?? { name, field, points: [], min: value, max: value };
          counter.points.push({ time, value, index });
          counter.min = Math.min(counter.min, value);
          counter.max = Math.max(counter.max, value);
          series.set(key, counter);
        }
      }
      return { index, event, start: time, end: finish, row: 0, kind };
    });
    const spanRows = stackSpans(events);
    const counters = [...series.values()];
    const height = LANE_HEADER + spanRows * ROW_HEIGHT + counters.length * COUNTER_HEIGHT + 8;
    const result = { tid: lane.tid, name: lane.name, top, height, spanRows, events, counters };
    top += height;
    return result;
  });
  if (!Number.isFinite(start)) start = 0;
  if (!Number.isFinite(end) || end <= start) end = start + 1;
  return { start, end, height: top, lanes };
}

export function formatTime(microseconds: number): string {
  return `${(microseconds / 1_000_000).toFixed(3)}s`;
}
