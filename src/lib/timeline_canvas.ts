import {
  LANE_HEADER,
  ROW_HEIGHT,
  eventColor,
  eventName,
  timeToX,
  counterPointY,
  formatRulerTime,
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
  chartBar: string;
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
  const { width, height, gutter, range, ticks, scrollTop, tracks, theme } = frame;
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
    const trackTop = track.top - scrollTop;
    const trackBottom = trackTop + track.height;
    if (trackBottom < 0 || trackTop > height) continue;
    const contentTop = trackTop + (track.kind === "events" ? LANE_HEADER : 0);
    const contentHeight = trackBottom - contentTop;
    if (track.kind === "events") {
      context.fillStyle = theme.muted;
      context.fillRect(gutter, trackTop, Math.max(1, width - gutter), LANE_HEADER);
      context.strokeStyle = theme.border;
      context.beginPath();
      context.moveTo(gutter, trackBottom - 0.5);
      context.lineTo(width, trackBottom - 0.5);
      context.stroke();
      if (track.collapsed) continue;
    }
    // Plots share one clip. DOM labels remain fixed in the gutter.
    context.save();
    context.beginPath();
    context.rect(gutter, contentTop, Math.max(1, width - gutter), contentHeight);
    context.clip();
    if (track.kind === "events") {
      drawEvents(context, track, trackTop, frame, hits);
    } else {
      drawCounterSeries(context, track, trackTop, frame, hits);
    }
    context.restore();
    context.fillStyle = theme.sidebar;
    context.fillRect(0, contentTop, gutter, contentHeight);
  }

  context.strokeStyle = theme.border;
  context.beginPath();
  context.moveTo(gutter - 0.5, 0);
  context.lineTo(gutter - 0.5, height);
  context.stroke();
  return hits;
}

function drawEvents(
  context: CanvasRenderingContext2D,
  track: Extract<TimelineTrack, { kind: "events" }>,
  trackTop: number,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { lane } = track;
  const { range } = frame;
  const plottedCounters = new Set(
    lane.counters.flatMap((counter) => counter.points.map((point) => point.index)),
  );
  for (const event of lane.events) {
    if (!hasEventTimes(event)) {
      continue;
    }
    if (event.end < range.start || event.start > range.end) {
      continue;
    }
    // Numeric counter events are drawn on their series; raw counters keep a mark.
    if (event.kind === "counter" && plottedCounters.has(event.index)) continue;
    hits.push(drawEvent(context, event, lane.tid, trackTop, frame));
  }
}

function hasEventTimes(
  event: EventGeometry,
): event is EventGeometry & { start: number; end: number } {
  return event.start !== null && event.end !== null;
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
    context.fillRect(eventLeft, eventTop, eventWidth, ROW_HEIGHT - 1);
    if (eventWidth > 28) {
      context.fillStyle = selected ? theme.background : theme.foreground;
      drawClippedLabel(
        context,
        eventName(event.event),
        eventLeft + 3,
        eventTop + 12,
        eventWidth - 5,
      );
    }
  } else {
    context.beginPath();
    context.moveTo(eventLeft, eventTop + 2);
    context.lineTo(eventLeft + 3, eventTop + 8);
    context.lineTo(eventLeft, eventTop + 14);
    context.lineTo(eventLeft - 3, eventTop + 8);
    context.closePath();
    context.fill();
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

function drawCounterSeries(
  context: CanvasRenderingContext2D,
  track: Extract<TimelineTrack, { kind: "counter" }>,
  trackTop: number,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { width, gutter, range, selection, theme } = frame;
  const { lane, counter } = track;
  const timeX = (time: number) => timeToX(time, range, width, gutter);
  const valueY = (value: number) => trackTop + counterPointY(counter, value);
  context.strokeStyle = theme.chartBar;
  context.lineWidth = 1.5;
  context.beginPath();
  for (const [pointIndex, point] of counter.points.entries()) {
    if (pointIndex === 0) {
      context.moveTo(timeX(point.time), valueY(point.value));
    } else {
      context.lineTo(timeX(point.time), valueY(point.value));
    }
  }
  context.stroke();
  for (const point of counter.points) {
    const pointX = timeX(point.time);
    if (pointX < gutter || pointX > width) continue;
    const pointY = valueY(point.value);
    // Cached builds can record only one sample. A moveTo-only path has no
    // stroke, so paint that sample even when it has not been selected.
    if (counter.points.length === 1) {
      context.fillStyle = theme.chartBar;
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
  context.lineWidth = 1;
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
