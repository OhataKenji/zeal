import {
  COUNTER_HEIGHT,
  COUNTER_HEADER,
  LANE_HEADER,
  ROW_HEIGHT,
  eventColor,
  eventTextColor,
  eventName,
  timeToX,
  counterPointY,
  formatRulerTime,
  formatCounter,
  type LaneGeometry,
  type TimeRange,
  type visibleTracks,
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
  tracks: ReturnType<typeof visibleTracks>;
  selection: { tid: number; index: number } | null;
}

function label(
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

export function drawRuler(context: CanvasRenderingContext2D, frame: TimelineAxis) {
  const { width, gutter, range } = frame;
  const plotWidth = Math.max(1, width - gutter);
  const x = (time: number) => timeToX(time, range, width, gutter);
  const { ticks } = frame;
  context.fillStyle = "#f1f3f5";
  context.fillRect(0, 0, width, 26);
  context.fillStyle = "#66717c";
  context.fillText("THREAD / TIME", 12, 17);
  context.save();
  context.beginPath();
  context.rect(gutter, 0, plotWidth, 26);
  context.clip();
  for (const time of ticks) {
    const px = x(time);
    context.strokeStyle = "#cbd0d5";
    context.beginPath();
    context.moveTo(px + 0.5, 18);
    context.lineTo(px + 0.5, 26);
    context.stroke();
    context.fillStyle = "#52606b";
    context.fillText(formatRulerTime(time, range), px + 4, 13);
  }
  context.restore();
}

function drawEvents(
  context: CanvasRenderingContext2D,
  lane: LaneGeometry,
  y: number,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { width, gutter, range, selection } = frame;
  const x = (time: number) => timeToX(time, range, width, gutter);
  const plottedCounters = new Set(
    lane.counters.flatMap((counter) => counter.points.map((point) => point.index)),
  );
  for (const event of lane.events) {
    if (event.start === null || event.end! < range.start || event.start > range.end) continue;
    // Numeric counters are already selectable on their series. Only raw
    // counters without numeric fields need a fallback mark in the span row.
    if (event.kind === "counter" && plottedCounters.has(event.index)) continue;
    const ex = Math.max(gutter, x(event.start));
    const ew = Math.max(1, Math.min(width, x(event.end!)) - ex);
    const ey = y + LANE_HEADER + event.row * ROW_HEIGHT;
    context.fillStyle = eventColor(event.event);
    if (event.kind === "span") {
      context.fillRect(ex, ey, ew, ROW_HEIGHT - 1);
      if (ew > 28) {
        context.fillStyle = eventTextColor(event.event);
        label(context, eventName(event.event), ex + 3, ey + 12, ew - 5);
      }
    } else {
      context.beginPath();
      context.moveTo(ex, ey + 2);
      context.lineTo(ex + 3, ey + 8);
      context.lineTo(ex, ey + 14);
      context.lineTo(ex - 3, ey + 8);
      context.closePath();
      context.fill();
    }
    if (selection?.tid === lane.tid && selection.index === event.index) {
      context.strokeStyle = "#111";
      context.lineWidth = 2;
      context.strokeRect(ex, ey + 0.5, Math.max(3, ew), ROW_HEIGHT - 2);
      context.lineWidth = 1;
    }
    hits.push({
      x: ex - 2,
      y: ey,
      width: Math.max(ew, 6),
      height: ROW_HEIGHT,
      tid: lane.tid,
      index: event.index,
    });
  }
}

function drawCounters(
  context: CanvasRenderingContext2D,
  lane: LaneGeometry,
  y: number,
  frame: TimelineFrame,
  hits: EventHit[],
) {
  const { width, gutter, range, selection } = frame;
  const x = (time: number) => timeToX(time, range, width, gutter);
  lane.counters.forEach((counter, ci) => {
    const pointY = (value: number) => y + counterPointY(lane, ci, value);
    context.strokeStyle = eventColor(lane.events[counter.points[0].index].event);
    context.lineWidth = 1.5;
    context.beginPath();
    counter.points.forEach((point, pi) => {
      if (pi === 0) context.moveTo(x(point.time), pointY(point.value));
      else context.lineTo(x(point.time), pointY(point.value));
    });
    context.stroke();
    context.fillStyle = context.strokeStyle;
    for (const point of counter.points) {
      const px = x(point.time);
      if (px < gutter || px > width) continue;
      const py = pointY(point.value);
      context.beginPath();
      context.arc(px, py, 2, 0, Math.PI * 2);
      context.fill();
      if (selection?.tid === lane.tid && selection.index === point.index) {
        context.save();
        context.strokeStyle = "#111";
        context.beginPath();
        context.arc(px, py, 4, 0, Math.PI * 2);
        context.stroke();
        context.restore();
      }
      hits.push({ x: px - 4, y: py - 4, width: 8, height: 8, tid: lane.tid, index: point.index });
    }
    context.lineWidth = 1;
  });
}

export function drawTimeline(context: CanvasRenderingContext2D, frame: TimelineFrame): EventHit[] {
  const { width, gutter, range } = frame;
  const plotWidth = Math.max(1, width - gutter);
  const x = (time: number) => timeToX(time, range, width, gutter);
  const { height, ticks, scrollTop, tracks } = frame;
  const hits: EventHit[] = [];
  // The ruler, phase strip, events and hit rectangles all use x(time).
  for (const time of ticks) {
    context.strokeStyle = "#e2e6e9";
    context.beginPath();
    context.moveTo(x(time) + 0.5, 0);
    context.lineTo(x(time) + 0.5, height);
    context.stroke();
  }
  for (const track of tracks) {
    const { lane } = track;
    const y = track.top - scrollTop;
    if (y + track.height < 0 || y > height) continue;
    context.fillStyle = "#e9ecef";
    context.fillRect(gutter, y, plotWidth, LANE_HEADER);
    context.strokeStyle = "#d7dce0";
    context.beginPath();
    context.moveTo(gutter, y + track.height - 0.5);
    context.lineTo(width, y + track.height - 0.5);
    context.stroke();
    if (track.collapsed) continue;
    context.save();
    context.beginPath();
    context.rect(gutter, y + LANE_HEADER, plotWidth, track.height - LANE_HEADER);
    context.clip();
    drawEvents(context, lane, y, frame, hits);
    if (lane.counters.length) {
      context.fillStyle = "#f3f5f6";
      context.fillRect(
        gutter,
        y + LANE_HEADER + lane.spanRows * ROW_HEIGHT,
        plotWidth,
        COUNTER_HEADER,
      );
    }
    if (track.countersExpanded) drawCounters(context, lane, y, frame, hits);
    context.restore();
    context.fillStyle = "#f8f9fa";
    context.fillRect(0, y + LANE_HEADER, gutter, track.height - LANE_HEADER);
    context.fillStyle = "#65717a";
    if (track.countersExpanded)
      lane.counters.forEach((counter, ci) => {
        const cy =
          y + LANE_HEADER + lane.spanRows * ROW_HEIGHT + COUNTER_HEADER + ci * COUNTER_HEIGHT;
        label(context, counter.name, 12, cy + 13, gutter - 20);
        label(
          context,
          `${counter.field} ${formatCounter(counter.min)}…${formatCounter(counter.max)}`,
          12,
          cy + 27,
          gutter - 20,
        );
      });
  }
  context.strokeStyle = "#cbd0d5";
  context.beginPath();
  context.moveTo(gutter - 0.5, 0);
  context.lineTo(gutter - 0.5, height);
  context.stroke();
  return hits;
}
