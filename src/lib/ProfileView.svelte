<script lang="ts">
  import type { Profile } from "./types";
  import {
    LANE_HEADER, eventColor,
    eventName, formatTime, layoutProfile,
    timeTicks, timeToX, visibleTracks, type EventGeometry, type LaneGeometry,
  } from "./timeline";

  import { drawRuler, drawTimeline, type EventHit } from "./timeline-canvas";

  let { profile }: { profile: Profile } = $props();
  const layout = $derived(layoutProfile(profile));
  const range = $derived({ start: layout.start, end: layout.end });
  let collapsed = $state.raw<ReadonlySet<number>>(new Set());
  const tracks = $derived(visibleTracks(layout.lanes, collapsed));
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
  let hits: EventHit[] = [];

  function x(time: number) { return timeToX(time, range, width, gutter); }
  function toggleThread(tid: number) {
    const next = new Set(collapsed);
    if (next.has(tid)) next.delete(tid); else next.add(tid);
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
    if (context) hits = drawTimeline(context, { width, height, gutter, range, ticks, scrollTop, tracks, selection });
  });

  function pointer(event: MouseEvent) {
    const box = canvas!.getBoundingClientRect();
    return { x: event.clientX - box.left, y: event.clientY - box.top };
  }
  function hit(px: number, py: number) {
    return hits.findLast((item) => px >= item.x && px <= item.x + item.width && py >= item.y && py <= item.y + item.height);
  }
  function selectEvent(event: MouseEvent) {
    const point = pointer(event);
    const found = hit(point.x, point.y);
    if (found) selection = { tid: found.tid, index: found.index };
  }
  function keydown(event: KeyboardEvent) {
    if (event.key !== "Escape" || !selection) return;
    selection = null;
    canvas?.focus();
    event.preventDefault();
  }
</script>

{#snippet threadLabels()}
  <div class="trace-thread-labels" style:width="{gutter}px">
    {#each tracks as track (track.lane)}
      <button style:top="{track.top}px" style:height="{LANE_HEADER}px" aria-expanded={!track.collapsed}
        title={`${track.lane.name || "thread"} · tid ${track.lane.tid}`}
        onclick={() => toggleThread(track.lane.tid)}>
        <span>{track.collapsed ? "▸" : "▾"}</span> {track.lane.name || "thread"}<small>{track.lane.tid}</small>
      </button>
    {/each}
  </div>
{/snippet}

{#snippet eventDetails(selected: EventGeometry, lane: LaneGeometry)}
  <section aria-labelledby="trace-event-title">
    <h2 class="panel-heading" id="trace-event-title">{eventName(selected.event)}</h2>
    <dl class="run-details">
      <dt>Thread</dt><dd>{lane.name || "thread"} · tid {lane.tid}</dd>
      <dt>Start</dt><dd>{selected.start === null ? "Unknown" : formatTime(selected.start)}</dd>
      {#if selected.kind === "span"}<dt>Duration</dt><dd>{formatTime(selected.end! - selected.start!)}</dd>{/if}
      {#if typeof selected.event.cat === "string"}<dt>Category</dt><dd>{selected.event.cat}</dd>{/if}
    </dl>
    <details class="trace-raw"><summary onkeydown={keydown}>Raw event fields</summary><pre>{JSON.stringify(selected.event, null, 2)}</pre></details>
  </section>
{/snippet}

<div>
  <div class="trace-toolbar">
    <strong>Timing</strong><span class="trace-count">{layout.lanes.length} threads</span>
    <div class="trace-actions">
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
        aria-label="Profile timeline. Select an event. Escape closes event details."
        onclick={selectEvent} onkeydown={keydown}></canvas>
      {@render threadLabels()}
    </div>
  </div>
  {#if selected && selectedLane}
    {@render eventDetails(selected, selectedLane)}
  {/if}
</div>
