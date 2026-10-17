<script lang="ts">
  import { tick } from "svelte";
  import type { Profile } from "./types";
  import {
    COUNTER_HEADER, LANE_HEADER, ROW_HEIGHT, eventColor,
    eventName, formatTime, layoutProfile, panRange, zoomRange,
    timeTicks, timeToX, xToTime, eventOffsetY, visibleTracks, type TimeRange,
  } from "./timeline";

  import { drawRuler, drawTimeline, type EventHit } from "./timeline-canvas";

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
  let pointerAnchor = 0.5;
  let drag: { id: number; x: number; y: number; range: TimeRange; moved: boolean } | null = null;
  let hits: EventHit[] = [];

  function x(time: number) { return timeToX(time, range, width, gutter); }
  function anchor(px: number) { return Math.max(0, Math.min(1, (xToTime(px, range, width, gutter) - range.start) / span)); }
  function fit() { windowRange = null; }
  function zoom(factor: number, anchor = 0.5) {
    windowRange = zoomRange(range, layout, factor, anchor);
  }
  function pan(delta: number) {
    windowRange = panRange(range, layout, delta);
  }
  function toggleThread(tid: number) {
    const next = new Set(collapsed);
    if (next.has(tid)) next.delete(tid); else next.add(tid);
    collapsed = next;
  }
  function toggleCounters(tid: number) {
    const next = new Set(expandedCounters);
    if (next.has(tid)) next.delete(tid); else next.add(tid);
    expandedCounters = next;
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

  $effect(() => {
    if (!ruler || width <= gutter) return;
    const context = prepare(ruler, 26);
    if (context) drawRuler(context, { width, gutter, range, ticks });
  });

  $effect(() => {
    if (!canvas || width <= gutter || height <= 0) return;
    const context = prepare(canvas, height);
    if (context) hits = drawTimeline(context, { width, height, gutter, range, ticks, scrollTop, tracks, selection });
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
    }
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
    canvas?.focus();
  }
  function keydown(event: KeyboardEvent) {
    if (event.key === "Escape" && selection) {
      closeSelection();
      event.preventDefault();
      return;
    }
    if (event.target !== canvas || event.ctrlKey || event.metaKey || event.altKey) return;
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

<div>
  <div class="trace-toolbar">
    <strong>Timing</strong><span class="trace-count">{layout.lanes.length} threads</span><span class="trace-input-hint">Drag to pan</span>
    <div class="trace-actions">
      <button class="btn" onclick={fit} title="Fit the complete profile (F)">Fit</button>
      <button class="btn" onclick={() => zoom(1 / 1.5)} aria-label="Zoom out" title="Zoom out (S)">−</button>
      <button class="btn" onclick={() => zoom(1.5)} aria-label="Zoom in" title="Zoom in (W)">+</button>
      <button class="btn" onclick={() => { collapsed = collapsed.size === layout.lanes.length ? new Set() : new Set(layout.lanes.map((lane) => lane.tid)); }}>
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
  <div class="trace-viewport" bind:this={viewport} bind:clientWidth={width} bind:clientHeight={height}
    onscroll={() => { if (viewport) scrollTop = viewport.scrollTop; }}>
    <div class="trace-scroll-content" style:height="{Math.max(height, totalHeight)}px">
      <canvas bind:this={canvas} style:width="{width}px" style:height="{height}px" tabindex="0"
        aria-label="Profile timeline. Drag to pan. W/S zoom, A/D pan, F fit. Left/right selects adjacent events. Escape closes event details."
        onpointerdown={pointerDown} onpointermove={pointerMove} onpointerup={pointerUp}
        onpointercancel={cancelDrag} onlostpointercapture={cancelDrag} onkeydown={keydown}></canvas>
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
  {#if selected && selectedLane}
    <section aria-labelledby="trace-event-title">
      <h2 class="panel-heading" id="trace-event-title">{eventName(selected.event)}</h2>
      <dl class="run-details">
        <dt>Thread</dt><dd>{selectedLane.name || "thread"} · tid {selectedLane.tid}</dd>
        <dt>Start</dt><dd>{selected.start === null ? "Unknown" : formatTime(selected.start)}</dd>
        {#if selected.kind === "span"}<dt>Duration</dt><dd>{formatTime(selected.end! - selected.start!)}</dd>{/if}
        {#if typeof selected.event.cat === "string"}<dt>Category</dt><dd>{selected.event.cat}</dd>{/if}
      </dl>
      <details class="trace-raw"><summary onkeydown={keydown}>Raw event fields</summary><pre>{JSON.stringify(selected.event, null, 2)}</pre></details>
    </section>
  {/if}
</div>
