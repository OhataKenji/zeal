import type { Profile, TraceEvent } from "./types.ts";

/** Height of one event row in canvas coordinates. */
export const ROW_HEIGHT = 18;
/** Height of a thread label and event-row header. */
export const LANE_HEADER = 20;
/** Height of one counter row in canvas coordinates. */
export const COUNTER_HEIGHT = 42;

/** Row geometry for one trace event after parsing its timestamps. */
export interface EventGeometry {
  index: number;
  event: TraceEvent;
  start: number | null;
  end: number | null;
  row: number;
  kind: "span" | "mark" | "counter";
}

/** Numeric values sampled from one trace counter field. */
export interface CounterSeries {
  name: string;
  field: string;
  /** Time-sorted samples; viewport windowing depends on the order. */
  points: Array<{ time: number; value: number; index: number }>;
  min: number;
  max: number;
}

/** Event and counter rows for one thread in a profile. */
export interface LaneGeometry {
  tid: number;
  name: string;
  height: number;
  spanRows: number;
  /** Non-overlapping spans grouped by stacked row, each sorted by start for viewport culling. */
  rows: Array<EventGeometry & { start: number; end: number }>[];
  /** Timed non-span events not plotted as counters, sorted by start. */
  marks: Array<EventGeometry & { start: number; end: number }>;
  events: EventGeometry[];
  counters: CounterSeries[];
}

/** Full profile layout before viewport filtering or canvas drawing. */
export interface Timeline {
  start: number;
  end: number;
  lanes: LaneGeometry[];
}

interface TimelineTrackBase {
  lane: LaneGeometry;
  top: number;
  height: number;
}

/** A visible timeline row: either the thread's events or one counter series. */
export type TimelineTrack = TimelineTrackBase &
  ({ kind: "events"; collapsed: boolean } | { kind: "counter"; counter: CounterSeries });

/** Maps a counter value into the vertical range reserved for a counter row. */
export function counterPointY(counter: CounterSeries, value: number): number {
  return (
    COUNTER_HEIGHT -
    5 -
    ((value - counter.min) / (counter.max - counter.min || 1)) * (COUNTER_HEIGHT - 12)
  );
}

// A raw event may contribute to several counter series. Reveal its first plotted
// series; counters without numeric samples stay on their original event track.
/** Finds the visible row and canvas Y coordinate for a trace event. */
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

/** Narrows arbitrary trace values to finite numbers. */
export function numeric(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Supplies a readable label when a trace event has no string name. */
export function eventName(event: TraceEvent): string {
  return typeof event.name === "string" ? event.name : "(unnamed event)";
}

// Categorical trace palette rotated through 8 hues at L=0.74, C=0.14 in OKLCH
// matching the bright, luminous tone of the analytics bar chart.
const EVENT_COLORS = [
  "#47c58c", // 160° (Mint / Success)
  "#00c3d5", // 205° (Cyan / Sky)
  "#60b0ff", // 250° (Blue)
  "#b298f9", // 295° (Purple / Violet)
  "#e486c6", // 340° (Rose)
  "#f7857d", // 25°  (Coral / Destructive)
  "#e29936", // 70°  (Amber / Warning)
  "#a8b540", // 115° (Lime / Chartreuse)
];

/** Assigns a stable palette color from the trace's color group or event name. */
export function eventColor(event: TraceEvent): string {
  const key = typeof event.cname === "string" ? event.cname : eventName(event);
  let hash = 2166136261;
  for (const char of key) {
    hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
  }
  return EVENT_COLORS[(hash >>> 0) % EVENT_COLORS.length];
}

// Use the shallowest free row. Sorting only the geometry keeps raw file order.
function hasSpanTimes(
  event: EventGeometry,
): event is EventGeometry & { start: number; end: number } {
  return event.kind === "span" && event.start !== null && event.end !== null;
}

/** Binary min-heap ordered by `compare`; backs sweep-line bookkeeping. */
class MinHeap<T> {
  private items: T[] = [];
  private compare: (a: T, b: T) => number;
  constructor(compare: (a: T, b: T) => number) {
    this.compare = compare;
  }

  get size(): number {
    return this.items.length;
  }

  peek(): T {
    return this.items[0];
  }

  push(item: T): void {
    const items = this.items;
    items.push(item);
    let i = items.length - 1;
    while (i > 0) {
      const parent = (i - 1) >> 1;
      if (this.compare(items[i], items[parent]) >= 0) break;
      [items[i], items[parent]] = [items[parent], items[i]];
      i = parent;
    }
  }

  pop(): T {
    const top = this.peek();
    const last = this.items.pop();
    if (this.items.length && last !== undefined) {
      this.items[0] = last;
      this.siftDown();
    }
    return top;
  }

  private siftDown(): void {
    const items = this.items;
    let i = 0;
    for (;;) {
      let child = i * 2 + 1;
      if (child + 1 < items.length && this.compare(items[child + 1], items[child]) < 0) {
        child++;
      }
      if (child >= items.length || this.compare(items[i], items[child]) <= 0) break;
      [items[i], items[child]] = [items[child], items[i]];
      i = child;
    }
  }
}

// Process spans in start order: once a row's last span ends it stays reusable,
// so a min-heap of free rows finds the shallowest fit without scanning rows.
function stackSpans(events: EventGeometry[]): {
  spanRows: number;
  rows: Array<EventGeometry & { start: number; end: number }>[];
} {
  const busy = new MinHeap<{ end: number; row: number }>((a, b) => a.end - b.end);
  const free = new MinHeap<number>((a, b) => a - b);
  const rows: Array<EventGeometry & { start: number; end: number }>[] = [];
  for (const span of events
    .filter(hasSpanTimes)
    .sort((a, b) => a.start - b.start || b.end - a.end || a.index - b.index)) {
    while (busy.size && busy.peek().end <= span.start) {
      free.push(busy.pop().row);
    }
    const row = free.size ? free.pop() : rows.length;
    span.row = row;
    (rows[row] ??= []).push(span);
    busy.push({ end: span.end, row });
  }
  return { spanRows: Math.max(rows.length, 1), rows };
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
  const finish = time === null ? null : time + (kind === "span" ? (duration ?? 0) : 0);
  return { index, event, start: time, end: finish, row: 0, kind };
}

function counterSeries(events: EventGeometry[]): CounterSeries[] {
  const series = new Map<string, CounterSeries>();
  for (const { event, index, start, kind } of events) {
    if (kind !== "counter" || start === null || !event.args || typeof event.args !== "object") {
      continue;
    }
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
  return sortCounterPoints(series);
}

// Viewport culling and a time-ordered polyline both want sorted samples.
function sortCounterPoints(series: Map<string, CounterSeries>): CounterSeries[] {
  for (const counter of series.values()) {
    counter.points.sort((a, b) => a.time - b.time);
  }
  return [...series.values()];
}

function isDrawableMark(
  event: EventGeometry,
  plotted: ReadonlySet<number>,
): event is EventGeometry & { start: number; end: number } {
  return (
    event.kind !== "span" && event.start !== null && event.end !== null && !plotted.has(event.index)
  );
}

/** Adds event timing and counter geometry while preserving each lane's event order. */
export function layoutProfile(profile: Profile): Timeline {
  const lanes = profile.lanes.map((lane) => {
    const events = lane.events.map(eventGeometry);
    const counters = counterSeries(events);
    const plotted = new Set(
      counters.flatMap((counter) => counter.points.map((point) => point.index)),
    );
    const { spanRows, rows } = stackSpans(events);
    const marks = events
      .filter((event) => isDrawableMark(event, plotted))
      .sort((a, b) => a.start - b.start || a.index - b.index);
    const height = LANE_HEADER + spanRows * ROW_HEIGHT + 8;
    return { tid: lane.tid, name: lane.name, height, spanRows, rows, marks, events, counters };
  });
  const { start, end } = profileDomain(lanes, profile.phases);
  return { start, end, lanes };
}

/** Bounds the timeline by phase and event times, keeping a non-empty range. */
function profileDomain(lanes: LaneGeometry[], phases: Profile["phases"]): TimeRange {
  const times = [
    ...phases.flatMap((phase) => [phase.startUs, phase.endUs]),
    ...lanes.flatMap((lane) => lane.events.flatMap(eventTimes)),
  ];
  let start = Infinity;
  let end = -Infinity;
  for (const time of times) {
    start = Math.min(start, time);
    end = Math.max(end, time);
  }
  if (!Number.isFinite(start)) start = 0;
  if (!Number.isFinite(end) || end <= start) end = start + 1;
  return { start, end };
}

function eventTimes(event: EventGeometry): number[] {
  return [event.start, event.end].filter((time): time is number => time !== null);
}

/** Formats trace microseconds using seconds, milliseconds, or microseconds. */
export function formatTime(microseconds: number): string {
  const magnitude = Math.abs(microseconds);
  if (magnitude >= 1_000_000) return `${Number((microseconds / 1_000_000).toFixed(3))}s`;
  if (magnitude >= 1_000) return `${Number((microseconds / 1_000).toFixed(3))}ms`;
  return `${Number(microseconds.toFixed(2))}µs`;
}

/** Start and end of a timeline viewport, in trace microseconds. */
export interface TimeRange {
  start: number;
  end: number;
}

/** Formats a ruler tick using a unit suitable for the full visible range. */
export function formatRulerTime(time: number, range: TimeRange): string {
  const span = range.end - range.start;
  const divisor = span >= 1_000_000 ? 1_000_000 : span >= 1_000 ? 1_000 : 1;
  return `${Number((time / divisor).toFixed(3))}${divisor === 1_000_000 ? "s" : divisor === 1_000 ? "ms" : "µs"}`;
}

/** Formats a counter value with a compact SI suffix when needed. */
export function formatCounter(value: number): string {
  const magnitude = Math.abs(value);
  return magnitude >= 1_000_000 || (magnitude > 0 && magnitude < 0.001)
    ? value.toExponential(3)
    : String(Number(value.toPrecision(4)));
}

/** Projects trace time to canvas X, reserving the left gutter for labels. */
export function timeToX(time: number, range: TimeRange, width: number, gutter: number): number {
  return gutter + ((time - range.start) / (range.end - range.start)) * Math.max(1, width - gutter);
}

/** Converts canvas X back to trace time using the same gutter and range. */
export function xToTime(x: number, range: TimeRange, width: number, gutter: number): number {
  return range.start + ((x - gutter) / Math.max(1, width - gutter)) * (range.end - range.start);
}

/** Clamps a viewport to its domain while preserving its width when possible. */
export function clampRange(range: TimeRange, domain: TimeRange): TimeRange {
  const total = domain.end - domain.start;
  const span = Math.max(Math.min(total, 1), Math.min(total, range.end - range.start));
  const start = Math.max(domain.start, Math.min(domain.end - span, range.start));
  return { start, end: start + span };
}

/** Zooms around a normalized anchor and clamps the result to the domain. */
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

/** Pans a viewport by a time delta, clamped to the profile domain. */
export function panRange(range: TimeRange, domain: TimeRange, delta: number): TimeRange {
  return clampRange({ start: range.start + delta, end: range.end + delta }, domain);
}

/** Returns the thumb width and left offset for a horizontal timeline range. */
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

/** Places a viewport at a normalized scrollbar position within the domain. */
export function scrollRange(range: TimeRange, domain: TimeRange, position: number): TimeRange {
  const span = range.end - range.start;
  const start =
    domain.start + (domain.end - domain.start - span) * Math.max(0, Math.min(1, position));
  return { start, end: start + span };
}

/** Generates bounded, strictly increasing ruler ticks for the visible range. */
export function timeTicks(range: TimeRange, width: number): number[] {
  const target = (range.end - range.start) / Math.max(1, width / 90);
  const base = 10 ** Math.floor(Math.log10(target));
  const step = ([1, 2, 5, 10].find((factor) => factor * base >= target) ?? 10) * base;
  const ticks: number[] = [];
  const first = Math.ceil(range.start / step) * step;
  // Absolute timestamps may exceed integer precision at a narrow zoom. An
  // incrementing absolute tick index could then never advance. Bound work by
  // viewport width and emit only representable, strictly increasing times.
  const limit = Math.max(2, Math.ceil(width / 90) + 2);
  for (let offset = 0; offset < limit; offset++) {
    const time = first + offset * step;
    if (time > range.end) break;
    if (time >= range.start && (!ticks.length || time > ticks[ticks.length - 1])) {
      ticks.push(time || 0);
    }
  }
  return ticks;
}

// Reference viewers list counters alphabetically by series name, then field.
// The source thread breaks ties so same-named series stay stable per lane.
function compareCounterTracks(
  a: Extract<TimelineTrack, { kind: "counter" }>,
  b: Extract<TimelineTrack, { kind: "counter" }>,
): number {
  return (
    a.counter.name.localeCompare(b.counter.name) ||
    a.counter.field.localeCompare(b.counter.field) ||
    a.lane.tid - b.lane.tid
  );
}

/**
 * Projects a profile into event and counter rows visible to the current view.
 * Collapsed threads keep their headers; counters get independent rows.
 */
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
  // Counters head the chart like the reference viewers. Display order stays
  // independent of the profile's original lane and event order.
  return [...counters, ...events].map((track) => {
    const positioned = { ...track, top };
    top += track.height;
    return positioned;
  });
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// Trace timestamp and duration fields read better formatted than as raw µs.
const TRACE_TIME_FIELDS = new Set(["ts", "tts", "dur", "tdur"]);

function fieldValue(key: string, value: unknown): string {
  const time = numeric(value);
  if (time !== null && TRACE_TIME_FIELDS.has(key)) return formatTime(time);
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value) ?? String(value);
}

/** Flattens a trace event into `[field, value]` rows for the inspector table. */
export function eventFields(event: TraceEvent): Array<[string, string]> {
  const rows: Array<[string, string]> = [];
  for (const [key, value] of Object.entries(event)) {
    if (isPlainObject(value)) {
      for (const [field, inner] of Object.entries(value)) {
        rows.push([`${key}.${field}`, fieldValue(`${key}.${field}`, inner)]);
      }
    } else {
      rows.push([key, fieldValue(key, value)]);
    }
  }
  return rows;
}
