<script lang="ts">
  import { tick } from "svelte";
  import type { Profile } from "./types";
  import { COUNTER_HEIGHT, LANE_HEADER, ROW_HEIGHT, eventColor, eventName, formatTime, layoutProfile, type EventGeometry } from "./timeline";

  let { profile }: { profile: Profile } = $props();
  const layout = $derived(layoutProfile(profile));
  let canvas: HTMLCanvasElement;
  let viewport: HTMLDivElement;
  let width = $state(600);
  let height = $state(360);
  let scrollTop = $state(0);
  let zoom = $state(1);
  let position = $state(0);
  let laneIndex = $state(0);
  let eventIndex = $state(0);
  let hover = $state<EventGeometry | null>(null);
  const lane = $derived(layout.lanes[laneIndex]);
  const selected = $derived(lane?.events[eventIndex]);
  const range = $derived((layout.end - layout.start) / zoom);
  const from = $derived(layout.start + (layout.end - layout.start - range) * position / 100);
  const to = $derived(from + range);
  const gutter = 180;
  const plotWidth = $derived(Math.max(1, width - gutter - 12));
  const shown = $derived(hover ?? selected);
  const rawText = $derived(shown ? JSON.stringify(shown.event, null, 2) : "");

  function x(time: number) { return gutter + (time - from) / range * plotWidth; }
  let hits: { x: number; y: number; w: number; h: number; lane: number; event: number }[] = [];

  $effect(() => {
    if (!canvas) return;
    const context = canvas.getContext("2d");
    if (!context) return;
    const ratio = window.devicePixelRatio || 1;
    canvas.width = width * ratio;
    canvas.height = height * ratio;
    context.scale(ratio, ratio);
    context.fillStyle = "#fff";
    context.fillRect(0, 0, width, height);
    context.font = "11px Menlo, monospace";
    hits = [];
    layout.lanes.forEach((current, li) => {
      const y = current.top - scrollTop;
      if (y + current.height < 0 || y > height) return;
      context.fillStyle = "#eee";
      context.fillRect(0, y, width, LANE_HEADER);
      context.fillStyle = "#333";
      context.fillText(`${current.name || "thread"} · tid ${current.tid}`, 6, y + 16, width - 12);
      context.save();
      context.beginPath();
      context.rect(gutter, y + LANE_HEADER, plotWidth, current.height - LANE_HEADER);
      context.clip();
      for (const event of current.events) {
        if (event.start === null || event.end! < from || event.start > to) continue;
        const ex = Math.max(gutter, x(event.start));
        const ew = Math.max(3, Math.min(width - 12, x(event.end!)) - ex);
        const ey = y + LANE_HEADER + event.row * ROW_HEIGHT + 2;
        context.fillStyle = eventColor(event.event);
        if (event.kind === "span") {
          context.fillRect(ex, ey, ew, ROW_HEIGHT - 4);
          if (ew > 36) {
            context.fillStyle = "#222";
            context.fillText(eventName(event.event), ex + 3, ey + 12, ew - 6);
          }
        } else {
          context.beginPath();
          context.moveTo(ex, ey);
          context.lineTo(ex + 4, ey + 8);
          context.lineTo(ex, ey + 16);
          context.lineTo(ex - 4, ey + 8);
          context.closePath();
          context.fill();
        }
        if (li === laneIndex && event.index === eventIndex) {
          context.strokeStyle = "#111";
          context.lineWidth = 2;
          context.strokeRect(ex - 1, ey - 1, ew + 2, ROW_HEIGHT - 2);
        }
        hits.push({ x: ex - 3, y: ey, w: Math.max(ew, 8), h: ROW_HEIGHT, lane: li, event: event.index });
      }
      current.counters.forEach((counter, ci) => {
        const cy = y + LANE_HEADER + current.spanRows * ROW_HEIGHT + ci * COUNTER_HEIGHT;
        context.strokeStyle = eventColor(current.events[counter.points[0].index].event);
        context.lineWidth = 2;
        context.beginPath();
        counter.points.forEach((point, pi) => {
          const px = x(point.time);
          const py = cy + 52 - (point.value - counter.min) / (counter.max - counter.min || 1) * 32;
          if (pi === 0) context.moveTo(px, py); else context.lineTo(px, py);
          if (px >= gutter && px <= width - 12) {
            hits.push({ x: px - 5, y: py - 5, w: 10, h: 10, lane: li, event: point.index });
          }
        });
        context.stroke();
        context.fillStyle = context.strokeStyle;
        for (const point of counter.points) {
          const px = x(point.time);
          if (px < gutter || px > width - 12) continue;
          const py = cy + 52 - (point.value - counter.min) / (counter.max - counter.min || 1) * 32;
          context.beginPath();
          context.arc(px, py, 2, 0, Math.PI * 2);
          context.fill();
        }
        context.fillStyle = "#444";
        context.fillText(`${counter.name} / ${counter.field} (${counter.min}…${counter.max})`, gutter + 4, cy + 13, plotWidth - 8);
      });
      context.restore();
    });
  });

  function hit(event: PointerEvent) {
    const box = canvas.getBoundingClientRect();
    const px = event.clientX - box.left;
    const py = event.clientY - box.top;
    return hits.findLast((item) => px >= item.x && px <= item.x + item.w && py >= item.y && py <= item.y + item.h);
  }

  async function reveal() {
    hover = null;
    await tick();
    if (!lane || !viewport) return;
    const selectedY = lane.top + LANE_HEADER + (selected?.row ?? 0) * ROW_HEIGHT;
    if (selectedY < viewport.scrollTop || selectedY + ROW_HEIGHT > viewport.scrollTop + height) {
      viewport.scrollTop = Math.max(lane.top, selectedY - LANE_HEADER);
    }
    if (selected?.start !== null && selected?.start !== undefined && (selected.start < from || selected.start > to)) {
      position = Math.max(0, Math.min(100, (selected.start - layout.start - range / 2) / (layout.end - layout.start - range || 1) * 100));
    }
  }

  function moveEvent(delta: number) {
    eventIndex = Math.max(0, Math.min((lane?.events.length ?? 1) - 1, eventIndex + delta));
    void reveal();
  }
</script>

<div class="trace-controls">
  <label>zoom <input aria-label="Timeline zoom" type="range" min="1" max="100" bind:value={zoom} /></label>
  <label>position <input aria-label="Timeline position" type="range" min="0" max="100" step="0.1" bind:value={position} disabled={zoom === 1} /></label>
  <button class="btn" onclick={() => { zoom = 1; position = 0; }}>fit</button>
</div>
<p class="label">{formatTime(from)} — {formatTime(to)} · {layout.lanes.length} threads</p>
<div class="phases" aria-label="Build phase markers">
  {#each profile.phases as phase}
    {#if phase.endUs >= from && phase.startUs <= to}
      <div class="phase" style:left="{Math.max(0, (phase.startUs - from) / range * 100)}%" style:width="{Math.max(0.2, (Math.min(to, phase.endUs) - Math.max(from, phase.startUs)) / range * 100)}%" style:background={eventColor({ name: phase.name })} title={`${phase.name}: ${formatTime(phase.startUs)} — ${formatTime(phase.endUs)}`}>
        {phase.name}
      </div>
    {/if}
  {/each}
</div>
<details class="phase-key">
  <summary>phase markers ({profile.phases.length})</summary>
  {#each profile.phases as phase}
    <p>{phase.name} <span class="dim">{formatTime(phase.startUs)} → {formatTime(phase.endUs)}</span></p>
  {/each}
</details>
<p class="label">Skymeld では analysis と execution が重なります。帯は元の marker 名を示します。thread の時間の合計は run の経過時間ではありません。</p>
<div class="trace-viewport" bind:this={viewport} bind:clientWidth={width} bind:clientHeight={height} onscroll={() => { scrollTop = viewport.scrollTop; }}>
  <div style:height="{Math.max(height, layout.height)}px">
    <canvas bind:this={canvas} style:width="{width}px" style:height="{height}px" tabindex="0" aria-label="Profile timeline. 左右キーで同じthreadの前後のeventを選択。下のthread選択とevent番号でも全eventを確認できます。" title={rawText}
      onpointermove={(event) => { const found = hit(event); hover = found ? layout.lanes[found.lane].events[found.event] : null; }}
      onpointerleave={() => { hover = null; }}
      onpointerdown={(event) => { const found = hit(event); if (found) { laneIndex = found.lane; eventIndex = found.event; } canvas.focus(); }}
      onkeydown={(event) => { if (event.key === "ArrowRight" || event.key === "ArrowLeft") { event.preventDefault(); moveEvent(event.key === "ArrowRight" ? 1 : -1); } }}>
    </canvas>
  </div>
</div>
<div class="trace-controls">
  <label>thread <select class="select" bind:value={laneIndex} onchange={() => { eventIndex = 0; void reveal(); }}>
    {#each layout.lanes as item, index (item.tid)}<option value={index}>{item.name || "thread"} · tid {item.tid}</option>{/each}
  </select></label>
  <button class="btn" disabled={eventIndex <= 0} onclick={() => moveEvent(-1)}>←</button>
  <label>event <input class="input event-number" type="number" min="1" max={lane?.events.length ?? 1} value={eventIndex + 1} onchange={(event) => { eventIndex = Math.max(0, Math.min((lane?.events.length ?? 1) - 1, Number(event.currentTarget.value) - 1)); void reveal(); }} /> / {lane?.events.length ?? 0}</label>
  <button class="btn" disabled={eventIndex >= (lane?.events.length ?? 1) - 1} onclick={() => moveEvent(1)}>→</button>
</div>
<p class="label">event は file の順序です。時間のない event も番号で選択できます。span は段に積み、instant とその他の event は mark、counter は名前と field ごとの系列で表示します。</p>
{#if shown}<pre class="event-detail" aria-label="Raw event fields">{rawText}</pre>{/if}
