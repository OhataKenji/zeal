import type { Profile, TraceEvent } from "./types.ts";

export const ROW_HEIGHT = 18;
export const LANE_HEADER = 20;

export interface EventGeometry {
  index: number;
  event: TraceEvent;
  start: number | null;
  end: number | null;
  row: number;
  kind: "span" | "mark";
}

export interface LaneGeometry {
  tid: number;
  name: string;
  height: number;
  spanRows: number;
  events: EventGeometry[];
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
  collapsed: boolean;
};

export function eventPosition(tracks: TimelineTrack[], tid: number, eventIndex: number) {
  const track = tracks.find((candidate) => candidate.lane.tid === tid);
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
export function eventColor(event: TraceEvent): string {
  const key = typeof event.cname === "string" ? event.cname : eventName(event);
  let hash = 2166136261;
  for (const char of key) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
  return `hsl(${205 + ((hash >>> 0) % 18)} 14% ${75 + ((hash >>> 0) % 13)}%)`;
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
  const kind = event.ph === "X" && duration !== null && duration >= 0 ? "span" : "mark";
  const finish = time === null ? null : time + (kind === "span" ? duration! : 0);
  return { index, event, start: time, end: finish, row: 0, kind };
}

export function layoutProfile(profile: Profile): Timeline {
  const lanes = profile.lanes.map((lane) => {
    const events = lane.events.map(eventGeometry);
    const spanRows = stackSpans(events);
    const height = LANE_HEADER + spanRows * ROW_HEIGHT + 8;
    return { tid: lane.tid, name: lane.name, height, spanRows, events };
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
): TimelineTrack[] {
  let top = 0;
  return [...lanes]
    .sort((a, b) => a.tid - b.tid)
    .map((lane) => {
      const closed = collapsed.has(lane.tid);
      const height = closed ? LANE_HEADER : lane.height;
      const track = { lane, top, height, collapsed: closed };
      top += height;
      return track;
    });
}
