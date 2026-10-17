import type { Profile, TraceEvent } from "./types.ts";

export const ROW_HEIGHT = 18;
export const LANE_HEADER = 20;
export const COUNTER_HEIGHT = 42;
export const COUNTER_HEADER = 20;

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

export function counterPointY(lane: LaneGeometry, seriesIndex: number, value: number): number {
  const counter = lane.counters[seriesIndex];
  const top =
    LANE_HEADER + lane.spanRows * ROW_HEIGHT + COUNTER_HEADER + seriesIndex * COUNTER_HEIGHT;
  return (
    top +
    COUNTER_HEIGHT -
    5 -
    ((value - counter.min) / (counter.max - counter.min || 1)) * (COUNTER_HEIGHT - 12)
  );
}

export function eventOffsetY(lane: LaneGeometry, eventIndex: number): number {
  for (const [seriesIndex, counter] of lane.counters.entries()) {
    const point = counter.points.find((candidate) => candidate.index === eventIndex);
    if (point) return counterPointY(lane, seriesIndex, point.value);
  }
  return LANE_HEADER + lane.events[eventIndex].row * ROW_HEIGHT + ROW_HEIGHT / 2;
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

function pushHeap<T>(heap: T[], value: T, before: (a: T, b: T) => boolean) {
  heap.push(value);
  let index = heap.length - 1;
  while (index > 0) {
    const parent = Math.floor((index - 1) / 2);
    if (!before(heap[index], heap[parent])) break;
    [heap[index], heap[parent]] = [heap[parent], heap[index]];
    index = parent;
  }
}

function popHeap<T>(heap: T[], before: (a: T, b: T) => boolean): T {
  const first = heap[0];
  const last = heap.pop()!;
  if (heap.length) heap[0] = last;
  let index = 0;
  while (2 * index + 1 < heap.length) {
    let child = 2 * index + 1;
    if (child + 1 < heap.length && before(heap[child + 1], heap[child])) child++;
    if (!before(heap[child], heap[index])) break;
    [heap[index], heap[child]] = [heap[child], heap[index]];
    index = child;
  }
  return first;
}

// Release ended spans, then use the shallowest free row. Sorting only the
// geometry keeps nested spans compact in O(n log n), preserving raw file order.
function stackSpans(events: EventGeometry[]): number {
  const active: { end: number; row: number }[] = [];
  const free: number[] = [];
  const byEnd = (a: { end: number }, b: { end: number }) => a.end < b.end;
  const byRow = (a: number, b: number) => a < b;
  let rows = 0;
  for (const span of events
    .filter((e) => e.kind === "span" && e.start !== null)
    .sort((a, b) => a.start! - b.start! || b.end! - a.end! || a.index - b.index)) {
    while (active.length && active[0].end <= span.start!) {
      pushHeap(free, popHeap(active, byEnd).row, byRow);
    }
    const row = free.length ? popHeap(free, byRow) : rows++;
    span.row = row;
    pushHeap(active, { end: span.end!, row }, byEnd);
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
    const height =
      LANE_HEADER +
      spanRows * ROW_HEIGHT +
      (counters.length ? COUNTER_HEADER : 0) +
      counters.length * COUNTER_HEIGHT +
      8;
    const result = { tid: lane.tid, name: lane.name, top, height, spanRows, events, counters };
    top += height;
    return result;
  });
  if (!Number.isFinite(start)) start = 0;
  if (!Number.isFinite(end) || end <= start) end = start + 1;
  return { start, end, height: top, lanes };
}

export function formatTime(microseconds: number): string {
  const magnitude = Math.abs(microseconds);
  if (magnitude >= 1_000_000) return `${Number((microseconds / 1_000_000).toFixed(3))}s`;
  if (magnitude >= 1_000) return `${Number((microseconds / 1_000).toFixed(3))}ms`;
  return `${Number(microseconds.toFixed(2))}µs`;
}

export interface TimeRange {
  start: number;
  end: number;
}

export function formatRulerTime(time: number, range: TimeRange): string {
  const span = range.end - range.start;
  const divisor = span >= 1_000_000 ? 1_000_000 : span >= 1_000 ? 1_000 : 1;
  return `${Number((time / divisor).toFixed(3))}${divisor === 1_000_000 ? "s" : divisor === 1_000 ? "ms" : "µs"}`;
}

export function formatCounter(value: number): string {
  const magnitude = Math.abs(value);
  return magnitude >= 1_000_000 || (magnitude > 0 && magnitude < 0.001)
    ? value.toExponential(3)
    : String(Number(value.toPrecision(4)));
}

export function timeToX(time: number, range: TimeRange, width: number, gutter: number): number {
  return gutter + ((time - range.start) / (range.end - range.start)) * Math.max(1, width - gutter);
}

export function xToTime(x: number, range: TimeRange, width: number, gutter: number): number {
  return range.start + ((x - gutter) / Math.max(1, width - gutter)) * (range.end - range.start);
}

export function clampRange(range: TimeRange, domain: TimeRange): TimeRange {
  const total = domain.end - domain.start;
  const span = Math.max(Math.min(total, 1), Math.min(total, range.end - range.start));
  const start = Math.max(domain.start, Math.min(domain.end - span, range.start));
  return { start, end: start + span };
}

export function zoomRange(
  range: TimeRange,
  domain: TimeRange,
  factor: number,
  anchor = 0.5,
): TimeRange {
  const span = (range.end - range.start) / factor;
  const point = range.start + (range.end - range.start) * anchor;
  return clampRange({ start: point - span * anchor, end: point + span * (1 - anchor) }, domain);
}

export function panRange(range: TimeRange, domain: TimeRange, delta: number): TimeRange {
  return clampRange({ start: range.start + delta, end: range.end + delta }, domain);
}

export function timeTicks(range: TimeRange, width: number): number[] {
  const target = (range.end - range.start) / Math.max(1, width / 90);
  const base = 10 ** Math.floor(Math.log10(target));
  const step = [1, 2, 5, 10].find((factor) => factor * base >= target)! * base;
  const ticks: number[] = [];
  const first = Math.ceil(range.start / step) * step;
  // Absolute timestamps may exceed integer precision at a narrow zoom. An
  // incrementing absolute tick index could then never advance. Bound work by
  // viewport width and emit only representable, strictly increasing times.
  const limit = Math.max(2, Math.ceil(width / 90) + 2);
  for (let offset = 0; offset < limit; offset++) {
    const time = first + offset * step;
    if (time > range.end) break;
    if (time >= range.start && (!ticks.length || time > ticks[ticks.length - 1]))
      ticks.push(time || 0);
  }
  return ticks;
}

export function visibleTracks(
  lanes: LaneGeometry[],
  collapsed: ReadonlySet<number>,
  expandedCounters: ReadonlySet<number> = new Set(),
) {
  let top = 0;
  return lanes.map((lane, index) => {
    const closed = collapsed.has(lane.tid);
    const countersExpanded = expandedCounters.has(lane.tid);
    const height = closed
      ? LANE_HEADER
      : lane.height - (countersExpanded ? 0 : lane.counters.length * COUNTER_HEIGHT);
    const track = { lane, index, top, height, collapsed: closed, countersExpanded };
    top += height;
    return track;
  });
}
