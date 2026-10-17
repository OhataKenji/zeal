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

export function eventColor(event: TraceEvent): string {
  const key = typeof event.cname === "string" ? event.cname : eventName(event);
  let hash = 2166136261;
  for (const char of key) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
  return `hsl(${(hash >>> 0) % 360} 42% 72%)`;
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
