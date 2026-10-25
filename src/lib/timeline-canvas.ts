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

export interface EventHit {
  x: number;
  y: number;
  width: number;
  height: number;
  tid: number;
  index: number;
}

interface TimelineAxis {
  width: number;
  gutter: number;
  range: TimeRange;
  ticks: number[];
}

interface TimelineFrame extends TimelineAxis {
  height: number;
  scrollTop: number;
  tracks: TimelineTrack[];
  selection: { tid: number; index: number } | null;
}

export function drawRuler(context: CanvasRenderingContext2D, frame: TimelineAxis) {
  const { width, gutter, range, ticks } = frame;
  const plotWidth = Math.max(1, width - gutter);
  const timeX = (time: number) => timeToX(time, range, width, gutter);
  context.fillStyle = "#f5f5f5";
  context.fillRect(0, 0, width, 26);
  context.fillStyle = "#737373";
  context.fillText("THREAD / TIME", 12, 17);
  context.save();
  context.beginPath();
  context.rect(gutter, 0, plotWidth, 26);
  context.clip();
  for (const time of ticks) {
    const tickX = timeX(time);
    context.strokeStyle = "#cbd0d5";
    context.beginPath();
    context.moveTo(tickX + 0.5, 18);
    context.lineTo(tickX + 0.5, 26);
    context.stroke();
    context.fillStyle = "#737373";
    context.fillText(formatRulerTime(time, range), tickX + 4, 13);
  }
  context.restore();
}

export function drawTimeline(context: CanvasRenderingContext2D, frame: TimelineFrame): EventHit[] {
  const { width, height, gutter, range, ticks, scrollTop, tracks } = frame;
  const hits: EventHit[] = [];

  // Grid, ruler, phase bands and event hit regions share the same time transform.
  for (const time of ticks) {
    const gridX = timeToX(time, range, width, gutter) + 0.5;
    context.strokeStyle = "#f0f0f0";
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
      context.fillStyle = "#f5f5f5";
      context.fillRect(gutter, trackTop, Math.max(1, width - gutter), LANE_HEADER);
      context.strokeStyle = "#d7dce0";
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
    if (track.kind === "events") drawEvents(context, track, trackTop, frame, hits);
    else drawCounterSeries(context, track, trackTop, frame, hits);
    context.restore();
    context.fillStyle = "#fafafa";
    context.fillRect(0, contentTop, gutter, contentHeight);
  }

  context.strokeStyle = "#cbd0d5";
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
    if (event.start === null || event.end! < range.start || event.start > range.end) continue;
    // Numeric counter events are drawn on their series; raw counters keep a mark.
    if (event.kind === "counter" && plottedCounters.has(event.index)) continue;
    hits.push(drawEvent(context, event, lane.tid, trackTop, frame));
  }
}

function drawEvent(
  context: CanvasRenderingContext2D,
  event: EventGeometry,
  tid: number,
  trackTop: number,
  frame: TimelineFrame,
): EventHit {
  const { width, gutter, range, selection } = frame;
  const timeX = (time: number) => timeToX(time, range, width, gutter);
  const eventLeft = Math.max(gutter, timeX(event.start!));
  const eventWidth = Math.max(1, Math.min(width, timeX(event.end!)) - eventLeft);
  const eventTop = trackTop + LANE_HEADER + event.row * ROW_HEIGHT;
  const selected = selection?.tid === tid && selection.index === event.index;
  context.fillStyle = selected ? "#262626" : eventColor(event.event);
  if (event.kind === "span") {
    context.fillRect(eventLeft, eventTop, eventWidth, ROW_HEIGHT - 1);
    if (eventWidth > 28) {
      context.fillStyle = selected ? "#fff" : "#262626";
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
    context.strokeStyle = "#262626";
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
  const { width, gutter, range, selection } = frame;
  const { lane, counter } = track;
  const timeX = (time: number) => timeToX(time, range, width, gutter);
  const valueY = (value: number) => trackTop + counterPointY(counter, value);
  context.strokeStyle = "#525252";
  context.lineWidth = 1.5;
  context.beginPath();
  for (const [pointIndex, point] of counter.points.entries()) {
    if (pointIndex === 0) context.moveTo(timeX(point.time), valueY(point.value));
    else context.lineTo(timeX(point.time), valueY(point.value));
  }
  context.stroke();
  context.fillStyle = context.strokeStyle;
  for (const point of counter.points) {
    const pointX = timeX(point.time);
    if (pointX < gutter || pointX > width) continue;
    const pointY = valueY(point.value);
    context.beginPath();
    context.arc(pointX, pointY, 2, 0, Math.PI * 2);
    context.fill();
    if (selection?.tid === lane.tid && selection.index === point.index) {
      context.save();
      context.strokeStyle = "#262626";
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
