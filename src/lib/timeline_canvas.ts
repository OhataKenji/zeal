import {
  COUNTER_HEIGHT,
  LANE_HEADER,
  ROW_HEIGHT,
  eventColor,
  eventName,
  timeToX,
  counterPointY,
  formatRulerTime,
  type CounterSeries,
  type EventGeometry,
  type TimeRange,
  type TimelineTrack,
} from "./timeline.ts";

/** Canvas rectangle used to map pointer input back to a trace event. */
export interface EventHit {
  x: number;
  y: number;
  width: number;
  height: number;
  tid: number;
  index: number;
}

/** Palette used to draw the timeline without depending on application CSS. */
export interface TimelineTheme {
  background: string;
  foreground: string;
  muted: string;
  mutedForeground: string;
  border: string;
  sidebar: string;
}

/** Stable color distinguishing one counter series from another. */
function counterColor(counter: CounterSeries): string {
  return eventColor({ name: `${counter.name}·${counter.field}` });
}

interface TimelineAxis {
  width: number;
  gutter: number;
  range: TimeRange;
  ticks: number[];
  theme: TimelineTheme;
}

interface TimelineFrame extends TimelineAxis {
  height: number;
  scrollTop: number;
  tracks: TimelineTrack[];
  selection: { tid: number; index: number } | null;
}

/** Draws ruler ticks and labels in the fixed-height canvas header. */
export function drawRuler(context: CanvasRenderingContext2D, frame: TimelineAxis) {
  const { width, gutter, range, ticks, theme } = frame;
  const plotWidth = Math.max(1, width - gutter);
  const timeX = (time: number) => timeToX(time, range, width, gutter);
  context.fillStyle = theme.muted;
  context.fillRect(0, 0, width, 26);
  context.save();
  context.beginPath();
  context.rect(gutter, 0, plotWidth, 26);
  context.clip();
  for (const time of ticks) {
    const tickX = timeX(time);
    context.strokeStyle = theme.border;
    context.beginPath();
    context.moveTo(tickX + 0.5, 18);
    context.lineTo(tickX + 0.5, 26);
    context.stroke();
    context.fillStyle = theme.mutedForeground;
    context.fillText(formatRulerTime(time, range), tickX + 4, 13);
  }
  context.restore();
}

/**
 * Draws visible profile tracks. `scrollTop` is subtracted from row positions;
 * the label gutter stays fixed while plots use the shared time transform.
 */
export function drawTimeline(context: CanvasRenderingContext2D, frame: TimelineFrame): EventHit[] {
  const { width, height, gutter, range, ticks, tracks, theme } = frame;
  const hits: EventHit[] = [];

  // Grid, ruler, phase bands and event hit regions share the same time transform.
  for (const time of ticks) {
    const gridX = timeToX(time, range, width, gutter) + 0.5;
    context.strokeStyle = theme.muted;
    context.beginPath();
    context.moveTo(gridX, 0);
    context.lineTo(gridX, height);
    context.stroke();
  }
  for (const track of tracks) {
    drawTrack(context, track, frame, hits);
  }

  context.strokeStyle = theme.border;
  context.beginPath();
  context.moveTo(gutter - 0.5, 0);
  context.lineTo(gutter - 0.5, height);
  context.stroke();
  return hits;
}

function drawTrack(
  context: CanvasRenderingContext2D,
  track: TimelineTrack,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { height, scrollTop } = frame;
  const trackTop = track.top - scrollTop;
  const trackBottom = trackTop + track.height;
  if (trackBottom < 0 || trackTop > height) return;
  if (track.kind === "events") {
    drawEventsTrack(context, track, trackTop, trackBottom, frame, hits);
  } else {
    drawCounterTrack(context, track, trackTop, trackBottom, frame, hits);
  }
}

function drawEventsTrack(
  context: CanvasRenderingContext2D,
  track: Extract<TimelineTrack, { kind: "events" }>,
  trackTop: number,
  trackBottom: number,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { width, gutter, theme } = frame;
  context.fillStyle = theme.muted;
  context.fillRect(gutter, trackTop, Math.max(1, width - gutter), LANE_HEADER);
  context.strokeStyle = theme.border;
  context.beginPath();
  context.moveTo(gutter, trackBottom - 0.5);
  context.lineTo(width, trackBottom - 0.5);
  context.stroke();
  if (track.collapsed) return;
  const contentTop = trackTop + LANE_HEADER;
  // Plots share one clip. DOM labels remain fixed in the gutter.
  context.save();
  context.beginPath();
  context.rect(gutter, contentTop, Math.max(1, width - gutter), trackBottom - contentTop);
  context.clip();
  drawEvents(context, track, trackTop, frame, hits);
  context.restore();
  context.fillStyle = theme.sidebar;
  context.fillRect(0, contentTop, gutter, trackBottom - contentTop);
}

function drawCounterTrack(
  context: CanvasRenderingContext2D,
  track: Extract<TimelineTrack, { kind: "counter" }>,
  trackTop: number,
  trackBottom: number,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { width, gutter, theme } = frame;
  context.save();
  context.beginPath();
  context.rect(gutter, trackTop, Math.max(1, width - gutter), trackBottom - trackTop);
  context.clip();
  drawCounterSeries(context, track, trackTop, frame, hits);
  context.restore();
  context.fillStyle = theme.sidebar;
  context.fillRect(0, trackTop, gutter, trackBottom - trackTop);
}

/** First index whose key is >= target; a row's non-overlapping spans sort by end too. */
function lowerBound<T>(items: T[], target: number, key: (item: T) => number): number {
  let lo = 0;
  let hi = items.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (key(items[mid]) < target) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  return lo;
}

// Rows and marks are pre-sorted by start, so binary search limits each frame
// to the events overlapping the viewport instead of scanning the lane.
function drawEvents(
  context: CanvasRenderingContext2D,
  track: Extract<TimelineTrack, { kind: "events" }>,
  trackTop: number,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { lane } = track;
  const { range } = frame;
  for (const row of lane.rows) {
    let i = lowerBound(row, range.start, (span) => span.end);
    for (; i < row.length && row[i].start <= range.end; i++) {
      hits.push(drawEvent(context, row[i], lane.tid, trackTop, frame));
    }
  }
  const from = lowerBound(lane.marks, range.start, (mark) => mark.start);
  for (let i = from; i < lane.marks.length && lane.marks[i].start <= range.end; i++) {
    hits.push(drawEvent(context, lane.marks[i], lane.tid, trackTop, frame));
  }
}

function drawEvent(
  context: CanvasRenderingContext2D,
  event: EventGeometry & { start: number; end: number },
  tid: number,
  trackTop: number,
  frame: TimelineFrame,
): EventHit {
  const { width, gutter, range, selection, theme } = frame;
  const timeX = (time: number) => timeToX(time, range, width, gutter);
  const eventLeft = Math.max(gutter, timeX(event.start));
  const eventWidth = Math.max(1, Math.min(width, timeX(event.end)) - eventLeft);
  const eventTop = trackTop + LANE_HEADER + event.row * ROW_HEIGHT;
  const selected = selection?.tid === tid && selection.index === event.index;
  context.fillStyle = selected ? theme.foreground : eventColor(event.event);
  if (event.kind === "span") {
    drawSpanBar(context, eventName(event.event), eventLeft, eventTop, eventWidth, selected, theme);
  } else {
    drawMarkDiamond(context, eventLeft, eventTop);
  }
  if (selected) {
    context.strokeStyle = theme.foreground;
    context.lineWidth = 2;
    context.strokeRect(eventLeft, eventTop + 0.5, Math.max(3, eventWidth), ROW_HEIGHT - 2);
    context.lineWidth = 1;
  }
  return {
    x: eventLeft - 2,
    y: eventTop,
    width: Math.max(eventWidth, 6),
    height: ROW_HEIGHT,
    tid,
    index: event.index,
  };
}

function drawSpanBar(
  context: CanvasRenderingContext2D,
  name: string,
  left: number,
  top: number,
  width: number,
  selected: boolean,
  theme: TimelineTheme,
) {
  context.fillRect(left, top, width, ROW_HEIGHT - 1);
  if (width <= 28) return;
  context.fillStyle = theme.background;
  drawClippedLabel(context, name, left + 3, top + 12, width - 5);
}

function drawMarkDiamond(context: CanvasRenderingContext2D, left: number, top: number) {
  context.beginPath();
  context.moveTo(left, top + 2);
  context.lineTo(left + 3, top + 8);
  context.lineTo(left, top + 14);
  context.lineTo(left - 3, top + 8);
  context.closePath();
  context.fill();
}

function drawCounterSeries(
  context: CanvasRenderingContext2D,
  track: Extract<TimelineTrack, { kind: "counter" }>,
  trackTop: number,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { width, gutter, range } = frame;
  const { counter } = track;
  const { points } = counter;
  const timeX = (time: number) => timeToX(time, range, width, gutter);
  const valueY = (value: number) => trackTop + counterPointY(counter, value);
  // Time-sorted points allow windowing; edge neighbors keep the polyline
  // entering and exiting the viewport exactly as a full pass would.
  const lo = lowerBound(points, range.start, (point) => point.time);
  let hi = lo;
  while (hi < points.length && points[hi].time <= range.end) hi++;
  const pathFrom = Math.max(0, lo - 1);
  const pathTo = Math.min(points.length, hi + 1);
  const traceLine = () => {
    context.beginPath();
    for (let i = pathFrom; i < pathTo; i++) {
      const point = points[i];
      if (i === pathFrom) {
        context.moveTo(timeX(point.time), valueY(point.value));
      } else {
        context.lineTo(timeX(point.time), valueY(point.value));
      }
    }
  };
  // Shade the area under the line down to the row bottom like Perfetto.
  const baseline = trackTop + COUNTER_HEIGHT - 1;
  context.save();
  context.globalAlpha = 0.4;
  context.fillStyle = counterColor(counter);
  traceLine();
  context.lineTo(timeX(points[pathTo - 1].time), baseline);
  context.lineTo(timeX(points[pathFrom].time), baseline);
  context.closePath();
  context.fill();
  context.restore();
  context.strokeStyle = counterColor(counter);
  context.lineWidth = 1.5;
  traceLine();
  context.stroke();
  for (let i = lo; i < hi; i++) {
    drawCounterPoint(context, track, points[i], timeX, valueY, frame, hits);
  }
  context.lineWidth = 1;
}

function drawCounterPoint(
  context: CanvasRenderingContext2D,
  track: Extract<TimelineTrack, { kind: "counter" }>,
  point: CounterSeries["points"][number],
  timeX: (time: number) => number,
  valueY: (value: number) => number,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { width, gutter, selection, theme } = frame;
  const { lane, counter } = track;
  const pointX = timeX(point.time);
  if (pointX < gutter || pointX > width) return;
  const pointY = valueY(point.value);
  // Cached builds can record only one sample. A moveTo-only path has no
  // stroke, so paint that sample even when it has not been selected.
  if (counter.points.length === 1) {
    context.fillStyle = counterColor(counter);
    context.beginPath();
    context.arc(pointX, pointY, 2, 0, Math.PI * 2);
    context.fill();
  }
  if (selection?.tid === lane.tid && selection.index === point.index) {
    context.save();
    context.strokeStyle = theme.foreground;
    context.beginPath();
    context.arc(pointX, pointY, 4, 0, Math.PI * 2);
    context.stroke();
    context.restore();
  }
  hits.push({
    x: pointX - 4,
    y: pointY - 4,
    width: 8,
    height: 8,
    tid: lane.tid,
    index: point.index,
  });
}

function drawClippedLabel(
  context: CanvasRenderingContext2D,
  text: string,
  left: number,
  baseline: number,
  available: number,
) {
  context.save();
  context.beginPath();
  context.rect(left, baseline - 12, Math.max(0, available), 15);
  context.clip();
  context.fillText(text, left, baseline);
  context.restore();
}
