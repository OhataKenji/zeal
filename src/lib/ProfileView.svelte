<script lang="ts">
  import { tick } from "svelte";
  import type { Profile } from "./types";
  import {
    COUNTER_HEIGHT, COUNTER_HEADER, LANE_HEADER, ROW_HEIGHT, eventColor, eventTextColor,
    eventName, formatTime, layoutProfile, panRange, zoomRange,
    timeTicks, timeToX, xToTime, counterPointY, eventOffsetY, formatRulerTime, formatCounter, visibleTracks, type TimeRange,
  } from "./timeline";

  let { profile }: { profile: Profile } = $props();
  const layout = $derived(layoutProfile(profile));
  let windowRange = $state.raw<TimeRange | null>(null);
  const range = $derived(windowRange ?? { start: layout.start, end: layout.end });
  const span = $derived(range.end - range.start);
  let collapsed = $state.raw<ReadonlySet<number>>(new Set());
  let expandedCounters = $state.raw<ReadonlySet<number>>(new Set());
  const tracks = $derived(visibleTracks(layout.lanes, collapsed, expandedCounters));
  const totalHeight = $derived(tracks.reduce((sum, track) => sum + track.height, 0));
  let selection = $state<{ tid: number; index: number } | null>(null);
  const selectedLane = $derived(layout.lanes.find((lane) => lane.tid === selection?.tid));
  const selected = $derived(selectedLane?.events[selection?.index ?? 0]);

  let canvas = $state<HTMLCanvasElement>();
  let ruler = $state<HTMLCanvasElement>();
  let viewport = $state<HTMLDivElement>();
  let width = $state(600);
  let height = $state(320);
  let scrollTop = $state(0);
  const gutter = 190;
  const plotWidth = $derived(Math.max(1, width - gutter));
  const ticks = $derived(timeTicks(range, plotWidth));
  let hover = $state<{ tid: number; index: number; x: number; y: number } | null>(null);
  const hovered = $derived(layout.lanes.find((lane) => lane.tid === hover?.tid)?.events[hover?.index ?? -1]);
  let pointerAnchor = 0.5;
  let drag: { id: number; x: number; y: number; range: TimeRange; moved: boolean } | null = null;
  let hits: { x: number; y: number; width: number; height: number; tid: number; index: number }[] = [];

  function x(time: number) { return timeToX(time, range, width, gutter); }
  function anchor(px: number) { return Math.max(0, Math.min(1, (xToTime(px, range, width, gutter) - range.start) / span)); }
  function fit() { windowRange = null; hover = null; }
  function zoom(factor: number, anchor = 0.5) {
    windowRange = zoomRange(range, layout, factor, anchor);
    hover = null;
  }
  function pan(delta: number) {
    windowRange = panRange(range, layout, delta);
    hover = null;
  }
  function toggleThread(tid: number) {
    const next = new Set(collapsed);
    if (next.has(tid)) next.delete(tid); else next.add(tid);
    collapsed = next;
    hover = null;
  }
  function toggleCounters(tid: number) {
    const next = new Set(expandedCounters);
    if (next.has(tid)) next.delete(tid); else next.add(tid);
    expandedCounters = next;
    hover = null;
  }

  function prepare(element: HTMLCanvasElement, canvasHeight: number) {
    const context = element.getContext("2d");
    if (!context) return null;
    const ratio = window.devicePixelRatio || 1;
    element.width = Math.max(1, width * ratio);
    element.height = Math.max(1, canvasHeight * ratio);
    context.scale(ratio, ratio);
    context.fillStyle = "#fff";
    context.fillRect(0, 0, width, canvasHeight);
    context.font = "10px Menlo, monospace";
    return context;
  }

  function label(context: CanvasRenderingContext2D, text: string, left: number, baseline: number, available: number) {
    context.save();
    context.beginPath();
    context.rect(left, baseline - 12, Math.max(0, available), 15);
    context.clip();
    context.fillText(text, left, baseline);
    context.restore();
  }

  $effect(() => {
    if (!ruler || width <= gutter) return;
    const context = prepare(ruler, 26);
    if (!context) return;
    context.fillStyle = "#f1f3f5";
    context.fillRect(0, 0, width, 26);
    context.fillStyle = "#66717c";
    context.fillText("THREAD / TIME", 12, 17);
    context.save();
    context.beginPath(); context.rect(gutter, 0, plotWidth, 26); context.clip();
    for (const time of ticks) {
      const px = x(time);
      context.strokeStyle = "#cbd0d5";
      context.beginPath(); context.moveTo(px + 0.5, 18); context.lineTo(px + 0.5, 26); context.stroke();
      context.fillStyle = "#52606b";
      context.fillText(formatRulerTime(time, range), px + 4, 13);
    }
    context.restore();
  });

  $effect(() => {
    if (!canvas || width <= gutter || height <= 0) return;
    const context = prepare(canvas, height);
    if (!context) return;
    hits = [];
    // The ruler, phase strip, events and hit rectangles all use x(time).
    for (const time of ticks) {
      context.strokeStyle = "#e2e6e9";
      context.beginPath(); context.moveTo(x(time) + 0.5, 0); context.lineTo(x(time) + 0.5, height); context.stroke();
    }
    for (const track of tracks) {
      const { lane } = track;
      const y = track.top - scrollTop;
      if (y + track.height < 0 || y > height) continue;
      context.fillStyle = "#e9ecef";
      context.fillRect(gutter, y, plotWidth, LANE_HEADER);
      context.strokeStyle = "#d7dce0";
      context.beginPath(); context.moveTo(gutter, y + track.height - 0.5); context.lineTo(width, y + track.height - 0.5); context.stroke();
      if (track.collapsed) continue;
      context.save();
      context.beginPath();
      context.rect(gutter, y + LANE_HEADER, plotWidth, track.height - LANE_HEADER);
      context.clip();
      const plottedCounters = new Set(lane.counters.flatMap((counter) => counter.points.map((point) => point.index)));
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
          context.moveTo(ex, ey + 2); context.lineTo(ex + 3, ey + 8);
          context.lineTo(ex, ey + 14); context.lineTo(ex - 3, ey + 8);
          context.closePath(); context.fill();
        }
        if (selection?.tid === lane.tid && selection.index === event.index) {
          context.strokeStyle = "#111";
          context.lineWidth = 2;
          context.strokeRect(ex, ey + 0.5, Math.max(3, ew), ROW_HEIGHT - 2);
          context.lineWidth = 1;
        }
        hits.push({ x: ex - 2, y: ey, width: Math.max(ew, 6), height: ROW_HEIGHT, tid: lane.tid, index: event.index });
      }
      if (lane.counters.length) {
        context.fillStyle = "#f3f5f6";
        context.fillRect(gutter, y + LANE_HEADER + lane.spanRows * ROW_HEIGHT, plotWidth, COUNTER_HEADER);
      }
      if (track.countersExpanded) lane.counters.forEach((counter, ci) => {
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
          context.beginPath(); context.arc(px, py, 2, 0, Math.PI * 2); context.fill();
          if (selection?.tid === lane.tid && selection.index === point.index) {
            context.save();
            context.strokeStyle = "#111";
            context.beginPath(); context.arc(px, py, 4, 0, Math.PI * 2); context.stroke();
            context.restore();
          }
          hits.push({ x: px - 4, y: py - 4, width: 8, height: 8, tid: lane.tid, index: point.index });
        }
        context.lineWidth = 1;
      });
      context.restore();
      context.fillStyle = "#f8f9fa";
      context.fillRect(0, y + LANE_HEADER, gutter, track.height - LANE_HEADER);
      context.fillStyle = "#65717a";
      if (track.countersExpanded) lane.counters.forEach((counter, ci) => {
        const cy = y + LANE_HEADER + lane.spanRows * ROW_HEIGHT + COUNTER_HEADER + ci * COUNTER_HEIGHT;
        label(context, counter.name, 12, cy + 13, gutter - 20);
        label(context, `${counter.field} ${formatCounter(counter.min)}…${formatCounter(counter.max)}`, 12, cy + 27, gutter - 20);
      });
    }
    context.strokeStyle = "#cbd0d5";
    context.beginPath(); context.moveTo(gutter - 0.5, 0); context.lineTo(gutter - 0.5, height); context.stroke();
  });

  function pointer(event: MouseEvent) {
    const box = canvas!.getBoundingClientRect();
    return { x: event.clientX - box.left, y: event.clientY - box.top };
  }
  function hit(px: number, py: number) {
    return hits.findLast((item) => px >= item.x && px <= item.x + item.width && py >= item.y && py <= item.y + item.height);
  }
  function pointerDown(event: PointerEvent) {
    if (event.button !== 0 || !canvas) return;
    const point = pointer(event);
    if (point.x < gutter) return;
    drag = { id: event.pointerId, x: event.clientX, y: event.clientY, range: { ...range }, moved: false };
    canvas.setPointerCapture(event.pointerId);
    canvas.focus();
    event.preventDefault();
  }
  function pointerMove(event: PointerEvent) {
    const point = pointer(event);
    pointerAnchor = anchor(point.x);
    if (drag?.id === event.pointerId) {
      const delta = event.clientX - drag.x;
      if (Math.hypot(delta, event.clientY - drag.y) > 3) drag.moved = true;
      if (drag.moved) windowRange = panRange(drag.range, layout, -delta / plotWidth * (drag.range.end - drag.range.start));
      hover = null;
      return;
    }
    const found = hit(point.x, point.y);
    hover = found ? { tid: found.tid, index: found.index, x: point.x, y: point.y } : null;
  }
  function pointerUp(event: PointerEvent) {
    if (!drag || drag.id !== event.pointerId) return;
    if (!drag.moved) {
      const point = pointer(event);
      const found = hit(point.x, point.y);
      if (found) selection = { tid: found.tid, index: found.index };
    }
    drag = null;
    if (canvas?.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
  }
  function cancelDrag() { drag = null; }

  async function moveEvent(delta: number) {
    if (!selection || !selectedLane) return;
    const lane = selectedLane;
    const tid = lane.tid;
    const selectedIndex = Math.max(0, Math.min(lane.events.length - 1, selection.index + delta));
    selection = { tid, index: selectedIndex };
    if (collapsed.has(tid)) toggleThread(tid);
    if (!expandedCounters.has(tid) && lane.counters.some((counter) => counter.points.some((point) => point.index === selectedIndex))) toggleCounters(tid);
    hover = null;
    await tick();
    if (selection?.tid !== tid || selection.index !== selectedIndex) return;
    const track = tracks.find((candidate) => candidate.lane.tid === tid);
    const event = lane.events[selection.index];
    if (!track || !event || !viewport) return;
    const selectedY = track.top + eventOffsetY(lane, selection.index);
    if (selectedY - ROW_HEIGHT / 2 < viewport.scrollTop || selectedY + ROW_HEIGHT / 2 > viewport.scrollTop + height) viewport.scrollTop = Math.max(track.top, selectedY - LANE_HEADER);
    if (event.start !== null && (event.start < range.start || event.start > range.end)) windowRange = panRange(range, layout, event.start - (range.start + range.end) / 2);
  }
  function closeSelection() {
    selection = null;
    hover = null;
    canvas?.focus();
  }
  function keydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      closeSelection();
      event.preventDefault();
      return;
    }
    if (event.currentTarget !== canvas || event.ctrlKey || event.metaKey || event.altKey) return;
    const key = event.key.toLowerCase();
    if (key === "w" || key === "+" || key === "=") zoom(1.5, pointerAnchor);
    else if (key === "s" || key === "-") zoom(1 / 1.5, pointerAnchor);
    else if (key === "a" || key === "d") pan(span * (key === "a" ? -0.15 : 0.15));
    else if (key === "f") fit();
    else if (key === "arrowleft" || key === "arrowright") void moveEvent(key === "arrowleft" ? -1 : 1);
    else return;
    event.preventDefault();
  }
</script>

<div class="trace-view">
  <div class="trace-toolbar">
    <strong>Timing</strong><span class="trace-count">{layout.lanes.length} threads</span><span class="trace-input-hint">Drag to pan</span>
    <div class="trace-actions">
      <button onclick={fit} title="Fit the complete profile (F)">Fit</button>
      <button onclick={() => zoom(1 / 1.5)} aria-label="Zoom out" title="Zoom out (S)">−</button>
      <button onclick={() => zoom(1.5)} aria-label="Zoom in" title="Zoom in (W)">+</button>
      <button onclick={() => { collapsed = collapsed.size === layout.lanes.length ? new Set() : new Set(layout.lanes.map((lane) => lane.tid)); }}>
        {collapsed.size === layout.lanes.length ? "Expand all" : "Collapse all"}
      </button>
    </div>
  </div>
  <div class="trace-axis"><canvas bind:this={ruler} style:width="{width}px" style:height="26px" aria-label="Timeline time ruler"></canvas></div>
  <div class="trace-phase-row" style:width="{width}px" title="Original phase markers. Skymeld may overlap analysis and execution.">
    <span style:width="{gutter}px">Build phases</span>
    <div class="trace-phases" style:left="{gutter}px" style:width="{plotWidth}px">
      {#each profile.phases as phase}
        {#if phase.endUs >= range.start && phase.startUs <= range.end}
          <div class="trace-phase" style:left="{x(Math.max(range.start, phase.startUs)) - gutter}px"
            style:width="{Math.max(1, x(Math.min(range.end, phase.endUs)) - x(Math.max(range.start, phase.startUs)))}px"
            style:background={eventColor({ name: phase.name })}
            title={`${phase.name}: ${formatTime(phase.startUs)} — ${formatTime(phase.endUs)}`}>{phase.name}</div>
        {/if}
      {/each}
    </div>
  </div>
  <div class="trace-body">
    <div class="trace-viewport" bind:this={viewport} bind:clientWidth={width} bind:clientHeight={height}
      onscroll={() => { if (viewport) scrollTop = viewport.scrollTop; hover = null; }}>
      <div class="trace-scroll-content" style:height="{Math.max(height, totalHeight)}px">
        <canvas bind:this={canvas} style:width="{width}px" style:height="{height}px" tabindex="0"
          aria-label="Profile timeline. Drag to pan. W/S zoom, A/D pan, F fit. Left/right selects adjacent events. Escape closes event details."
          onpointerdown={pointerDown} onpointermove={pointerMove} onpointerup={pointerUp}
          onpointercancel={cancelDrag} onlostpointercapture={cancelDrag}
          onpointerleave={() => hover = null} onkeydown={keydown}></canvas>
        <div class="trace-thread-labels" style:width="{gutter}px">
          {#each tracks as track (track.lane.tid)}
            <button style:top="{track.top}px" style:height="{LANE_HEADER}px" aria-expanded={!track.collapsed}
              title={`${track.lane.name || "thread"} · tid ${track.lane.tid}`}
              onclick={() => toggleThread(track.lane.tid)}>
              <span>{track.collapsed ? "▸" : "▾"}</span> {track.lane.name || "thread"}<small>{track.lane.tid}</small>
            </button>
            {#if !track.collapsed && track.lane.counters.length}
              <button class="trace-counter-toggle" style:top="{track.top + LANE_HEADER + track.lane.spanRows * ROW_HEIGHT}px"
                style:height="{COUNTER_HEADER}px" aria-expanded={track.countersExpanded}
                onclick={() => toggleCounters(track.lane.tid)}><span>{track.countersExpanded ? "▾" : "▸"}</span> Counters ({track.lane.counters.length})</button>
            {/if}
          {/each}
        </div>
      </div>
    </div>
    {#if !selection && hover && hovered}
      <div class="trace-tooltip" style:left="{Math.max(gutter, Math.min(width - 250, hover.x + 12))}px" style:top="{Math.max(0, Math.min(height - 74, hover.y + 12))}px">
        <strong>{eventName(hovered.event)}</strong>
        <span>{hovered.start === null ? "No timestamp" : formatTime(hovered.start)}{#if hovered.kind === "span"} · {formatTime(hovered.end! - hovered.start!)}{/if}</span>
        {#if typeof hovered.event.cat === "string"}<span>{hovered.event.cat}</span>{/if}
      </div>
    {/if}
    {#if selected && selectedLane}
      <dialog open class="trace-selection" aria-labelledby="trace-event-title" onkeydown={keydown}>
        <div class="trace-selection-title"><strong id="trace-event-title">{eventName(selected.event)}</strong><button class="btn" title="Close (Escape)" onclick={closeSelection}>Close</button></div>
        <dl>
          <dt>Thread</dt><dd>{selectedLane.name || "thread"} · tid {selectedLane.tid}</dd>
          <dt>Start</dt><dd>{selected.start === null ? "Unknown" : formatTime(selected.start)}</dd>
          {#if selected.kind === "span"}<dt>Duration</dt><dd>{formatTime(selected.end! - selected.start!)}</dd>{/if}
          {#if typeof selected.event.cat === "string"}<dt>Category</dt><dd>{selected.event.cat}</dd>{/if}
        </dl>
        <details class="trace-raw"><summary>Raw event fields</summary><pre>{JSON.stringify(selected.event, null, 2)}</pre></details>
      </dialog>
    {/if}
  </div>
</div>
