import type { Profile, TraceEvent } from "./types.ts";

export const ROW_HEIGHT = 18;
export const LANE_HEADER = 20;
export const COUNTER_HEIGHT = 42;

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
  height: number;
  spanRows: number;
  events: EventGeometry[];
  counters: CounterSeries[];
}

export interface Timeline {
  start: number;
  end: number;
  lanes: LaneGeometry[];
}

export type TimelineTrack = {
  lane: LaneGeometry;
  top: number;
  height: number;
} & ({ kind: "events"; collapsed: boolean } | { kind: "counter"; counter: CounterSeries });

export function counterPointY(counter: CounterSeries, value: number): number {
  return (
    COUNTER_HEIGHT -
    5 -
    ((value - counter.min) / (counter.max - counter.min || 1)) * (COUNTER_HEIGHT - 12)
  );
}

// A raw event may contribute to several counter series. Reveal its first plotted
// series; counters without numeric samples stay on their original event track.
export function eventPosition(tracks: TimelineTrack[], tid: number, eventIndex: number) {
  for (const track of tracks) {
    if (track.lane.tid !== tid || track.kind !== "counter") continue;
    const counter = track.counter;
    const point = counter.points.find((candidate) => candidate.index === eventIndex);
    if (point) return { track, y: track.top + counterPointY(counter, point.value) };
  }
  const track = tracks.find(
    (candidate) => candidate.kind === "events" && candidate.lane.tid === tid,
  );
  const event = track?.lane.events[eventIndex];
  if (!track || !event) return undefined;
  return { track, y: track.top + LANE_HEADER + event.row * ROW_HEIGHT + ROW_HEIGHT / 2 };
}

export function numeric(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

export function eventName(event: TraceEvent): string {
  return typeof event.name === "string" ? event.name : "(unnamed event)";
}

// Keep trace-provided color groups without reproducing another viewer's palette.
// AnalyticsView.svelte copies these hexes; terminal-style.ts derives ANSI hues.
const eventColors = [
  "#a7d6a9",
  "#b2dfdb",
  "#80c784",
  "#a5d5b0",
  "#ffdda6",
  "#c5afd4",
  "#99d4c6",
  "#b5dca8",
];

export function eventColor(event: TraceEvent): string {
  const key = typeof event.cname === "string" ? event.cname : eventName(event);
  let hash = 2166136261;
  for (const char of key) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
  return eventColors[(hash >>> 0) % eventColors.length];
}

// Use the shallowest free row. Sorting only the geometry keeps raw file order.
function stackSpans(events: EventGeometry[]): number {
  const rowEnds: number[] = [];
  for (const span of events
    .filter((e) => e.kind === "span" && e.start !== null)
    .sort((a, b) => a.start! - b.start! || b.end! - a.end! || a.index - b.index)) {
    const free = rowEnds.findIndex((end) => end <= span.start!);
    const row = free < 0 ? rowEnds.length : free;
    span.row = row;
    rowEnds[row] = span.end!;
  }
  return Math.max(rowEnds.length, 1);
}

function eventGeometry(event: TraceEvent, index: number): EventGeometry {
  const time = numeric(event.ts);
  const duration = numeric(event.dur);
  const kind =
    event.ph === "C"
      ? "counter"
      : event.ph === "X" && duration !== null && duration >= 0
        ? "span"
        : "mark";
  const finish = time === null ? null : time + (kind === "span" ? duration! : 0);
  return { index, event, start: time, end: finish, row: 0, kind };
}

function counterSeries(events: EventGeometry[]): CounterSeries[] {
  const series = new Map<string, CounterSeries>();
  for (const { event, index, start, kind } of events) {
    if (kind !== "counter" || start === null || !event.args || typeof event.args !== "object")
      continue;
    for (const [field, raw] of Object.entries(event.args)) {
      const value = numeric(raw);
      if (value === null) continue;
      const name = eventName(event);
      const key = JSON.stringify([name, field]);
      // Include zero so resource usage and constant positive values keep their
      // magnitude, matching BuildBuddy's baseline. Retain negative samples too.
      const counter = series.get(key) ?? { name, field, points: [], min: 0, max: 0 };
      counter.points.push({ time: start, value, index });
      counter.min = Math.min(counter.min, value);
      counter.max = Math.max(counter.max, value);
      series.set(key, counter);
    }
  }
  return [...series.values()];
}

export function layoutProfile(profile: Profile): Timeline {
  const lanes = profile.lanes.map((lane) => {
    const events = lane.events.map(eventGeometry);
    const spanRows = stackSpans(events);
    const counters = counterSeries(events);
    const height = LANE_HEADER + spanRows * ROW_HEIGHT + 8;
    return { tid: lane.tid, name: lane.name, height, spanRows, events, counters };
  });
  let start = Infinity;
  let end = -Infinity;
  const extend = (value: number) => {
    start = Math.min(start, value);
    end = Math.max(end, value);
  };
  profile.phases.forEach((phase) => {
    extend(phase.startUs);
    extend(phase.endUs);
  });
  for (const lane of lanes) {
    for (const event of lane.events) {
      if (event.start !== null) extend(event.start);
      if (event.end !== null) extend(event.end);
    }
  }
  if (!Number.isFinite(start)) start = 0;
  if (!Number.isFinite(end) || end <= start) end = start + 1;
  return { start, end, lanes };
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

export function scrollbarGeometry(range: TimeRange, domain: TimeRange, viewportWidth: number) {
  const span = range.end - range.start;
  const total = domain.end - domain.start;
  // Bound native scroll geometry even when a long trace is zoomed to microseconds.
  const width = Math.max(
    viewportWidth,
    Math.min(1_000_000, Math.ceil((viewportWidth * total) / span)),
  );
  const travel = total - span;
  const left = travel > 0 ? ((range.start - domain.start) / travel) * (width - viewportWidth) : 0;
  return { width, left };
}

export function scrollRange(range: TimeRange, domain: TimeRange, position: number): TimeRange {
  const span = range.end - range.start;
  const start =
    domain.start + (domain.end - domain.start - span) * Math.max(0, Math.min(1, position));
  return { start, end: start + span };
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

// Bazel counter names in the reference viewer's information order. The labels
// and original fields remain unchanged; unlisted metrics follow by name/field.
const counterOrder = [
  "action count",
  "action count (local)",
  "CPU usage (Bazel)",
  "Memory usage (Bazel)",
  "CPU usage (total)",
  "Memory usage (total)",
  "System load average",
  "Network Up usage (total)",
  "Network Down usage (total)",
];

function compareCounterTracks(
  a: Extract<TimelineTrack, { kind: "counter" }>,
  b: Extract<TimelineTrack, { kind: "counter" }>,
): number {
  const left = a.counter;
  const right = b.counter;
  const leftOrder = counterOrder.indexOf(left.name);
  const rightOrder = counterOrder.indexOf(right.name);
  return (
    (leftOrder < 0 ? counterOrder.length : leftOrder) -
      (rightOrder < 0 ? counterOrder.length : rightOrder) ||
    left.name.localeCompare(right.name) ||
    left.field.localeCompare(right.field) ||
    a.lane.tid - b.lane.tid
  );
}

export function visibleTracks(
  lanes: LaneGeometry[],
  collapsed: ReadonlySet<number>,
): TimelineTrack[] {
  const events: TimelineTrack[] = [...lanes]
    .sort((a, b) => a.tid - b.tid)
    .map((lane) => ({
      kind: "events",
      lane,
      top: 0,
      height: collapsed.has(lane.tid) ? LANE_HEADER : lane.height,
      collapsed: collapsed.has(lane.tid),
    }));
  const counters = lanes
    .flatMap((lane) =>
      lane.counters.map((counter) => ({
        kind: "counter" as const,
        lane,
        counter,
        top: 0,
        height: COUNTER_HEIGHT,
      })),
    )
    .sort(compareCounterTracks);
  let top = 0;
  // Display order is independent of the profile's original lane and event order.
  return [...events, ...counters].map((track) => {
    const positioned = { ...track, top };
    top += track.height;
    return positioned;
  });
}
