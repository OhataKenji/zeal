<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";
  import { tick } from "svelte";
  import type { Profile } from "./types";
  import {
    COUNTER_HEIGHT,
    LANE_HEADER,
    ROW_HEIGHT,
    eventName,
    formatCounter,
    formatTime,
    layoutProfile,
    panRange,
    zoomRange,
    timeTicks,
    timeToX,
    xToTime,
    eventPosition,
    visibleTracks,
    type TimeRange,
    type EventGeometry,
    type LaneGeometry,
  } from "./timeline";

  import { drawRuler, drawTimeline, type EventHit } from "./timeline-canvas";

  const phaseColors = ["#dce5df", "#b2dfdb", "#81c5b7", "#81bd86", "#c3e2c3"];

  let { profile }: { profile: Profile } = $props();
  const layout = $derived(layoutProfile(profile));
  let windowRange = $state.raw<TimeRange | null>(null);
  const range = $derived(
    windowRange ?? { start: layout.start, end: layout.end },
  );
  const span = $derived(range.end - range.start);
  let collapsed = $state.raw<ReadonlySet<number>>(new Set());
  const tracks = $derived(visibleTracks(layout.lanes, collapsed));
  const totalHeight = $derived(
    tracks.reduce((sum, track) => sum + track.height, 0),
  );
  let selection = $state<{ tid: number; index: number } | null>(null);
  const selectedLane = $derived(
    layout.lanes.find((lane) => lane.tid === selection?.tid),
  );
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
  let drag: {
    id: number;
    x: number;
    y: number;
    range: TimeRange;
    moved: boolean;
  } | null = null;
  let hits: EventHit[] = [];

  function x(time: number) {
    return timeToX(time, range, width, gutter);
  }
  function anchor(px: number) {
    return Math.max(
      0,
      Math.min(1, (xToTime(px, range, width, gutter) - range.start) / span),
    );
  }
  function fit() {
    windowRange = null;
  }
  function zoom(factor: number, anchor = 0.5) {
    windowRange = zoomRange(range, layout, factor, anchor);
  }
  function pan(delta: number) {
    windowRange = panRange(range, layout, delta);
  }
  function toggleThread(tid: number) {
    const next = new Set(collapsed);
    if (next.has(tid)) next.delete(tid);
    else next.add(tid);
    collapsed = next;
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
    if (context)
      hits = drawTimeline(context, {
        width,
        height,
        gutter,
        range,
        ticks,
        scrollTop,
        tracks,
        selection,
      });
  });

  function pointer(event: MouseEvent) {
    const box = canvas!.getBoundingClientRect();
    return { x: event.clientX - box.left, y: event.clientY - box.top };
  }
  function hit(px: number, py: number) {
    return hits.findLast(
      (item) =>
        px >= item.x &&
        px <= item.x + item.width &&
        py >= item.y &&
        py <= item.y + item.height,
    );
  }
  function pointerDown(event: PointerEvent) {
    if (event.button !== 0 || !canvas) return;
    const point = pointer(event);
    if (point.x < gutter) return;
    drag = {
      id: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      range: { ...range },
      moved: false,
    };
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
      if (drag.moved)
        windowRange = panRange(
          drag.range,
          layout,
          (-delta / plotWidth) * (drag.range.end - drag.range.start),
        );
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
    if (canvas?.hasPointerCapture(event.pointerId))
      canvas.releasePointerCapture(event.pointerId);
  }
  function cancelDrag() {
    drag = null;
  }

  async function moveEvent(delta: number) {
    if (!selection || !selectedLane) return;
    const lane = selectedLane;
    const lastIndex = lane.events.length - 1;
    const nextIndex = Math.max(0, Math.min(lastIndex, selection.index + delta));
    selection = { tid: lane.tid, index: nextIndex };
    const position = eventPosition(tracks, lane.tid, nextIndex);
    if (position?.track.kind === "events" && position.track.collapsed)
      toggleThread(lane.tid);

    // Expansion changes track geometry. A newer selection must win during that render.
    await tick();
    if (selection?.tid !== lane.tid || selection.index !== nextIndex) return;
    const revealed = eventPosition(tracks, lane.tid, nextIndex);
    const event = lane.events[nextIndex];
    if (!revealed || !viewport) return;
    const { track, y: eventY } = revealed;
    const eventTop = eventY - ROW_HEIGHT / 2;
    const eventBottom = eventY + ROW_HEIGHT / 2;
    if (
      eventTop < viewport.scrollTop ||
      eventBottom > viewport.scrollTop + height
    ) {
      viewport.scrollTop = Math.max(track.top, eventY - LANE_HEADER);
    }

    if (event.start === null) return;
    if (event.start < range.start || event.start > range.end) {
      const visibleCenter = (range.start + range.end) / 2;
      windowRange = panRange(range, layout, event.start - visibleCenter);
    }
  }
  function keydown(event: KeyboardEvent) {
    if (event.key === "Escape" && selection) {
      selection = null;
      canvas?.focus();
      event.preventDefault();
      return;
    }
    if (
      event.target !== canvas ||
      event.ctrlKey ||
      event.metaKey ||
      event.altKey
    )
      return;
    const key = event.key.toLowerCase();
    if (key === "w" || key === "+" || key === "=") zoom(1.5, pointerAnchor);
    else if (key === "s" || key === "-") zoom(1 / 1.5, pointerAnchor);
    else if (key === "a" || key === "d")
      pan(span * (key === "a" ? -0.15 : 0.15));
    else if (key === "f") fit();
    else if (key === "arrowleft" || key === "arrowright")
      void moveEvent(key === "arrowleft" ? -1 : 1);
    else return;
    event.preventDefault();
  }
</script>

{#snippet threadLabels()}
  <div class="trace-thread-labels" style:width="{gutter}px">
    {#each tracks as track (track.kind === "events" ? track.lane : track.counter)}
      {#if track.kind === "events"}
        <button
          style:top="{track.top}px"
          style:height="{LANE_HEADER}px"
          aria-expanded={!track.collapsed}
          title={`${track.lane.name || "thread"} · tid ${track.lane.tid}`}
          onclick={() => toggleThread(track.lane.tid)}
        >
          <span>{track.collapsed ? "▸" : "▾"}</span>
          {track.lane.name || "thread"}<small>{track.lane.tid}</small>
        </button>
      {:else}
        {@const counter = track.counter}
        <div
          class="trace-counter-label"
          style:top="{track.top}px"
          style:height="{COUNTER_HEIGHT}px"
          title={`${counter.name} · ${counter.field} · tid ${track.lane.tid}`}
        >
          <span>{counter.name} · tid {track.lane.tid}</span>
          <span
            >{counter.field}
            {formatCounter(counter.min)}…{formatCounter(counter.max)}</span
          >
        </div>
      {/if}
    {/each}
  </div>
{/snippet}

{#snippet eventDetails(selected: EventGeometry, lane: LaneGeometry)}
  <section aria-labelledby="trace-event-title">
    <h2 class="panel-heading" id="trace-event-title">
      {eventName(selected.event)}
    </h2>
    <dl class="run-details">
      <dt>Thread</dt>
      <dd>{lane.name || "thread"} · tid {lane.tid}</dd>
      <dt>Start</dt>
      <dd>
        {selected.start === null ? "Unknown" : formatTime(selected.start)}
      </dd>
      {#if selected.kind === "span"}<dt>Duration</dt>
        <dd class="event-duration">
          {formatTime(selected.end! - selected.start!)}
        </dd>{/if}
      {#if typeof selected.event.cat === "string"}<dt>Category</dt>
        <dd>{selected.event.cat}</dd>{/if}
    </dl>
    <details class="trace-raw" open>
      <summary onkeydown={keydown}>Raw event fields</summary>
      <pre>{JSON.stringify(selected.event, null, 2)}</pre>
    </details>
  </section>
{/snippet}

<div class="profile-view">
  <div class="trace-toolbar">
    <strong>Build phases</strong><span class="trace-count"
      >{layout.lanes.length} threads</span
    ><span class="trace-input-hint">Drag to pan</span>
    <div class="trace-actions">
      <Button
        variant="outline"
        size="sm"
        onclick={fit}
        title="Fit the complete profile (F)">Fit</Button
      >
      <Button
        variant="outline"
        size="sm"
        onclick={() => zoom(1 / 1.5)}
        aria-label="Zoom out"
        title="Zoom out (S)">−</Button
      >
      <Button
        variant="outline"
        size="sm"
        onclick={() => zoom(1.5)}
        aria-label="Zoom in"
        title="Zoom in (W)">+</Button
      >
      <Button
        variant="outline"
        size="sm"
        onclick={() => {
          collapsed =
            collapsed.size === layout.lanes.length
              ? new Set()
              : new Set(layout.lanes.map((lane) => lane.tid));
        }}
      >
        {collapsed.size === layout.lanes.length ? "Expand all" : "Collapse all"}
      </Button>
    </div>
  </div>
  <div class="trace-layout">
    <div class="trace-chart">
      <div class="trace-axis">
        <canvas
          bind:this={ruler}
          style:width="{width}px"
          style:height="26px"
          aria-label="Timeline time ruler"
        ></canvas>
      </div>
      <div
        class="trace-phase-row"
        style:width="{width}px"
        title="Original phase markers. Skymeld may overlap analysis and execution."
      >
        <span style:width="{gutter}px">Build phases</span>
        <div
          class="trace-phases"
          style:left="{gutter}px"
          style:width="{plotWidth}px"
        >
          {#each profile.phases as phase, index}
            {#if phase.endUs >= range.start && phase.startUs <= range.end}
              <div
                class="trace-phase"
                style:left="{x(Math.max(range.start, phase.startUs)) -
                  gutter}px"
                style:width="{Math.max(
                  1,
                  x(Math.min(range.end, phase.endUs)) -
                    x(Math.max(range.start, phase.startUs)),
                )}px"
                style:background={phaseColors[index % phaseColors.length]}
                title={`${phase.name}: ${formatTime(phase.startUs)} — ${formatTime(phase.endUs)}`}
              >
                {phase.name}
              </div>
            {/if}
          {/each}
        </div>
      </div>
      <div
        class="trace-viewport"
        bind:this={viewport}
        bind:clientWidth={width}
        bind:clientHeight={height}
        onscroll={() => {
          if (viewport) scrollTop = viewport.scrollTop;
        }}
      >
        <div
          class="trace-scroll-content"
          style:height="{Math.max(height, totalHeight)}px"
        >
          <canvas
            bind:this={canvas}
            style:width="{width}px"
            style:height="{height}px"
            tabindex="0"
            aria-label="Profile timeline. Drag to pan. W/S zoom, A/D pan, F fit. Left/right selects adjacent events. Escape closes event details."
            onpointerdown={pointerDown}
            onpointermove={pointerMove}
            onpointerup={pointerUp}
            onpointercancel={cancelDrag}
            onlostpointercapture={cancelDrag}
            onkeydown={keydown}
          ></canvas>
          {@render threadLabels()}
        </div>
      </div>
    </div>
    <aside class="trace-inspector">
      <h2 class="eyebrow">Selected event</h2>
      {#if selected && selectedLane}
        {@render eventDetails(selected, selectedLane)}
      {:else}<p class="dim">
          Select an event to inspect its timing and raw fields.
        </p>{/if}
    </aside>
  </div>
</div>
