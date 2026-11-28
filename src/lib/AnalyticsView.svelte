<script lang="ts">
  import type { Snippet } from "svelte";
  import Notice from "./Notice.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import type { HistoryFilter } from "./invocation";
  import {
    analyticsDateError,
    cacheHitRate,
    createAnalytics,
    emptyAnalytics,
    fmtMs,
    PLOT,
    segments,
    type AnalyticsFilter,
  } from "./analytics";
  import type { InvocationRecord } from "./types";

  const OUTCOMES = ["ok", "failed", "interrupted", "unknown"] satisfies readonly NonNullable<
    AnalyticsFilter["outcome"]
  >[];

  let { workspace, invocations, onselectday }: {
    workspace: string;
    invocations: InvocationRecord[];
    onselectday?: (day: string, filter: HistoryFilter) => void;
  } = $props();
  let view = $state.raw(emptyAnalytics());
  let controller = $state<ReturnType<typeof createAnalytics> | null>(null);
  let chartWidth = $state(0);
  let breakdown = $state<"result" | "workspace">("result");
  $effect(() => {
    // A workspace change discards both the filters and pending responses.
    void workspace;
    const next = createAnalytics(workspace, { invoke }, (value) => (view = value));
    controller = next;
    return next.dispose;
  });
  $effect(() => {
    void invocations;
    if (controller) void controller.load();
  });
  const dateError = $derived(analyticsDateError(view.filter));
  const data = $derived(view.data);
  const max = $derived(Math.max(1, ...(data?.days.map((d) => d.count) ?? [])));
  const maxTotal = $derived(
    Math.max(1, ...(data?.days.map((d) => d.totalMs) ?? [])),
  );
  const maxP95 = $derived(
    Math.max(1, ...(data?.days.map((d) => d.p95Ms) ?? [])),
  );
  const maxPhase = $derived(
    Math.max(1, ...(data?.days.map((d) => d.analysisMs + d.executionMs) ?? [])),
  );
  const hasPhaseData = $derived(data?.days.some((d) => d.phaseTimed > 0) ?? false);
  const hasCacheData = $derived(data?.days.some((d) => d.spawnTotal > 0) ?? false);
  const width = $derived(
    Math.max(640, (data?.days.length ?? 0) * 24 + 60, chartWidth),
  );
  const step = $derived((width - 70) / Math.max(1, data?.days.length ?? 1));
  const series = $derived.by((): Array<{ key: string; label: string; color: string }> => {
    if (breakdown === "result") {
      const colors = {
        ok: "success",
        failed: "destructive",
        interrupted: "warning",
        unknown: "muted-foreground",
      } as const;
      return OUTCOMES.map((kind) => ({
        key: kind,
        label: kind,
        color: `var(--${colors[kind]})`,
      }));
    }
    return (data?.workspaces ?? []).map((w, index) => ({
      key: w.id,
      label: w.path,
      color: `hsl(${(index * 137.508) % 360} 60% 50%)`,
    }));
  });
  const dayBars = $derived(
    (data?.days ?? []).map((day, index) => {
      let top = PLOT.baseline;
      const rects = series.flatMap((item) => {
        const count =
          breakdown === "result"
            ? day[item.key as (typeof OUTCOMES)[number]]
            : (day.workspaceCounts[item.key] ?? 0);
        const height = (count / max) * PLOT.height;
        if (height <= 0) return [];
        top -= height;
        return [{ ...item, count, x: PLOT.left + index * step, y: top, height }];
      });
      return { day, rects };
    }),
  );
  const edgeDays = $derived.by(() => {
    const days = data?.days ?? [];
    return [...new Set([0, days.length - 1])].flatMap((index) =>
      days[index] ? [{ index, day: days[index].day }] : [],
    );
  });
  function filter(patch: Partial<AnalyticsFilter>) {
    void controller?.load({ ...view.filter, ...patch });
  }
  function selectDay(day: string) {
    onselectday?.(day, view.filter.outcome === "failed" ? "failed" : "all");
  }
  function onDayKey(event: KeyboardEvent, day: string) {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      selectDay(day);
    }
  }
  type ChartKind = "count" | "total" | "duration" | "phase" | "cache";
  type AnalyticsDay = NonNullable<typeof data>["days"][number];
  function dayTip(kind: ChartKind, day: AnalyticsDay, index: number): string {
    const progress = day.day === data?.today ? " (in progress)" : "";
    const detail: Record<ChartKind, () => string> = {
      count: () => {
        const rects = dayBars[index]?.rects ?? [];
        const parts = rects.length > 0 ? ` (${rects.map((r) => `${r.label} ${r.count}`).join(", ")})` : "";
        return `${day.count} ${day.count === 1 ? "invocation" : "invocations"}${parts}${progress}`;
      },
      total: () => `${fmtMs(day.totalMs)}${progress}`,
      duration: () =>
        day.timed ? `p50 ${fmtMs(day.p50Ms)}, p95 ${fmtMs(day.p95Ms)}` : "no timed invocations",
      phase: () =>
        day.phaseTimed > 0
          ? `avg analysis ${fmtMs(day.analysisMs)}, avg execution ${fmtMs(day.executionMs)}`
          : "no phase metrics",
      cache: () => {
        const rate = cacheHitRate(day.spawnCacheHits, day.spawnTotal);
        return rate !== null
          ? `${rate}% spawn cache hits (${day.spawnCacheHits}/${day.spawnTotal})`
          : "no cache metrics";
      },
    };
    return `${day.day}: ${detail[kind]()} — open in History`;
  }
</script>

{#snippet chartSvg(
  label: string,
  yMax: string | number,
  yZero: string,
  tipKind: ChartKind,
  body: Snippet,
  describedBy?: string,
)}
  <div class="chart-scroll" bind:clientWidth={chartWidth}>
    <svg
      viewBox="0 0 {width} 240"
      style:width="{width}px"
      role="group"
      aria-label={label}
      aria-describedby={describedBy}
    >
      <title>{label}</title>
      <line x1="50" y1="20" x2="50" y2={PLOT.baseline} />
      <line x1="50" y1={PLOT.baseline} x2={width - 10} y2={PLOT.baseline} />
      <text x="44" y={PLOT.baseline - PLOT.height + 4} text-anchor="end">{yMax}</text>
      <text x="44" y={PLOT.baseline} text-anchor="end">{yZero}</text>
      <text x={width - 45} y="236">Date</text>
      {@render body()}
      {#each data?.days ?? [] as day, index}
        {#if day.count > 0}
          {@const tip = dayTip(tipKind, day, index)}
          <rect
            class="day-hit"
            x={PLOT.left + index * step - 1.5}
            y="20"
            width={Math.max(4, step)}
            height={PLOT.baseline - 20}
            role="button"
            tabindex="0"
            aria-label={tip}
            onclick={() => selectDay(day.day)}
            onkeydown={(e) => onDayKey(e, day.day)}
          >
            <title>{tip}</title>
          </rect>
        {/if}
      {/each}
      {#each edgeDays as { index, day }}
        <text x={PLOT.left + index * step} y="218" text-anchor={index === 0 ? "start" : "end"}
          >{day}</text
        >
      {/each}
    </svg>
  </div>
{/snippet}

<section class="analytics">
  <div class="view-head">
    <h1 class="view-title">Analytics</h1>
    <p class="dim hint">Click any day to inspect its slowest invocations in History.</p>
  </div>
  <div class="filters">
    <div class="date-filters">
      <label
        >Start date <input
          type="date"
          aria-label="Start date"
          aria-invalid={!!dateError}
          aria-describedby={dateError ? "analytics-date-error" : undefined}
          value={view.filter.start ?? ""}
          onchange={(e) => filter({ start: e.currentTarget.value || null })}
        /></label
      >
      <label
        >End date <input
          type="date"
          aria-label="End date"
          aria-invalid={!!dateError}
          aria-describedby={dateError ? "analytics-date-error" : undefined}
          value={view.filter.end ?? ""}
          onchange={(e) => filter({ end: e.currentTarget.value || null })}
        /></label
      >
      {#if dateError}
        <p id="analytics-date-error" class="error date-error" role="alert">
          {dateError}
        </p>
      {/if}
    </div>
    <label
      >Workspace <select
        aria-label="Workspace path"
        value={view.filter.workspaceId ?? ""}
        onchange={(e) =>
          filter({ workspaceId: e.currentTarget.value || null })}
      >
        <option value="">All workspaces</option>{#each data?.workspaces ?? [] as option (option.id)}<option
            value={option.id}>{option.path}</option
          >{/each}
      </select></label
    >
    <label
      >Command <input
        type="text"
        aria-label="Command"
        placeholder="All commands"
        value={view.filter.cmd ?? ""}
        onchange={(e) => filter({ cmd: e.currentTarget.value || null })}
      /></label
    >
    <label
      >Result <select
        aria-label="Result"
        value={view.filter.outcome ?? ""}
        onchange={(e) =>
          filter({
            // The current target is this view's outcome selector.
            outcome:
              (e.currentTarget.value as AnalyticsFilter["outcome"]) || null,
          })}
      >
        <option value="">All</option>{#each OUTCOMES as kind}<option>{kind}</option>{/each}
      </select></label
    >
    <label
      >Target patterns <input
        type="text"
        aria-label="Target patterns"
        placeholder="//app:server"
        value={view.filter.pattern ?? ""}
        onchange={(e) =>
          filter({ pattern: e.currentTarget.value || null })}
      /></label
    >
    <button type="button" class="button" onclick={() => controller?.load()}
      >Refresh</button
    >
  </div>
  {#if view.loading}<p role="status">Loading…</p>{/if}
  {#if view.error}<Notice tone="error">
      Unable to load analytics: {view.error}
    </Notice>{/if}
  {#if data}
    <section>
      <h2>Invocations</h2>
      <p class="dim">{data.filter.start} to {data.filter.end} · Invocations without a recorded start time are excluded.</p>
      <label class="breakdown"
        >Breakdown <select bind:value={breakdown} aria-label="Breakdown"
          ><option value="result">Result</option><option value="workspace">Workspace</option></select
        ></label
      >
      {#snippet countChart()}
        <desc id="analytics-daily-counts">
          {data.days
            .map(
              ({ day, count }) =>
                `${day}: ${count}${day === data.today ? " (in progress)" : ""}`,
            )
            .join("; ")}
        </desc>
        <text x="44" y="16" text-anchor="end">Count</text>
        {#each dayBars as { rects }}
          <g>
            {#each rects as rect}
              <rect
                style:fill={rect.color}
                x={rect.x}
                y={rect.y}
                width={Math.max(1, step - 3)}
                height={rect.height}
              />
            {/each}
          </g>
        {/each}
      {/snippet}
      {@render chartSvg("Invocations per day", max, "0", "count", countChart, "analytics-daily-counts")}
      <p class="legend">
        {#each series as item}
          <span><i class="sw" style:background={item.color}></i>{item.label}</span>
        {/each}
      </p>
    </section>
    <section>
      <h2>Total duration</h2>
      <p class="dim">Total duration per day</p>
      {#snippet totalChart()}
        {#each data.days as day, index}
          <rect
            x={PLOT.left + index * step}
            y={PLOT.baseline - (day.totalMs / maxTotal) * PLOT.height}
            width={Math.max(1, step - 3)}
            height={(day.totalMs / maxTotal) * PLOT.height}
          />
        {/each}
      {/snippet}
      {@render chartSvg("Total duration per day", fmtMs(maxTotal), "0", "total", totalChart)}
    </section>
    <section>
      <h2>Duration</h2>
      <p class="dim">Per-day invocation duration percentiles</p>
      {#snippet durationChart()}
        {#each segments(
          data.days.map((d) => (d.timed ? d.p95Ms : null)),
          maxP95,
          step,
        ) as points}
          <polyline class="line-p95" {points} />
        {/each}
        {#each segments(
          data.days.map((d) => (d.timed ? d.p50Ms : null)),
          maxP95,
          step,
        ) as points}
          <polyline class="line-p50" {points} />
        {/each}
        {#each data.days as day, index}
          {#if day.timed}
            <circle
              class="dot-p95"
              cx={PLOT.left + index * step + step / 2}
              cy={PLOT.baseline - (day.p95Ms / maxP95) * PLOT.height}
              r="2.5"
            />
            <circle
              class="dot-p50"
              cx={PLOT.left + index * step + step / 2}
              cy={PLOT.baseline - (day.p50Ms / maxP95) * PLOT.height}
              r="2.5"
            />
          {/if}
        {/each}
      {/snippet}
      {@render chartSvg("Per-day invocation duration percentiles", fmtMs(maxP95), "0", "duration", durationChart)}
      <p class="legend">
        <span><i class="sw sw-p50"></i>p50</span>
        <span><i class="sw sw-p95"></i>p95</span>
      </p>
    </section>
    {#if hasPhaseData}
      <section>
        <h2>Phase duration</h2>
        <p class="dim">Average analysis vs execution phase time per invocation</p>
        {#snippet phaseChart()}
          {#each data.days as day, index}
            {@const execH = (day.executionMs / maxPhase) * PLOT.height}
            {@const analysisH = (day.analysisMs / maxPhase) * PLOT.height}
            {#if execH > 0}
              <rect
                class="bar-execution"
                x={PLOT.left + index * step}
                y={PLOT.baseline - execH}
                width={Math.max(1, step - 3)}
                height={execH}
              />
            {/if}
            {#if analysisH > 0}
              <rect
                class="bar-analysis"
                x={PLOT.left + index * step}
                y={PLOT.baseline - execH - analysisH}
                width={Math.max(1, step - 3)}
                height={analysisH}
              />
            {/if}
          {/each}
        {/snippet}
        {@render chartSvg("Phase duration per day", fmtMs(maxPhase), "0", "phase", phaseChart)}
        <p class="legend">
          <span><i class="sw sw-analysis"></i>Analysis</span>
          <span><i class="sw sw-execution"></i>Execution</span>
        </p>
      </section>
    {/if}
    {#if hasCacheData}
      <section>
        <h2>Cache hit rate</h2>
        <p class="dim">Daily spawn cache hit percentage</p>
        {#snippet cacheChart()}
          <line
            class="grid-line"
            x1="50"
            y1={PLOT.baseline - PLOT.height / 2}
            x2={width - 10}
            y2={PLOT.baseline - PLOT.height / 2}
          />
          <text x="44" y={PLOT.baseline - PLOT.height / 2 + 4} text-anchor="end">50%</text>
          {#each segments(
            data.days.map((d) => cacheHitRate(d.spawnCacheHits, d.spawnTotal)),
            100,
            step,
          ) as points}
            <polyline class="line-cache" {points} />
          {/each}
          {#each data.days as day, index}
            {@const rate = cacheHitRate(day.spawnCacheHits, day.spawnTotal)}
            {#if rate !== null}
              <circle
                class="dot-cache"
                cx={PLOT.left + index * step + step / 2}
                cy={PLOT.baseline - (rate / 100) * PLOT.height}
                r="2.5"
              />
            {/if}
          {/each}
        {/snippet}
        {@render chartSvg("Cache hit rate per day", "100%", "0%", "cache", cacheChart)}
      </section>
    {/if}
    {#if data.patterns.length}
      <section>
        <h2>Target patterns</h2>
        <p class="dim">Complete pattern lists by total duration</p>
        <div class="table-scroll">
        <table class="patterns">
          <thead
            ><tr
              ><th>Target patterns</th><th>Invocations</th><th>Total</th><th>p50</th><th
                >p95</th
              ></tr
            ></thead
          >
          <tbody>
            {#each data.patterns as t}
              <tr class:active={view.filter.pattern === t.pattern}>
                <td
                  ><button
                    type="button"
                    class="target-link"
                    onclick={() =>
                      filter({
                        pattern:
                          view.filter.pattern === t.pattern ? null : t.pattern,
                      })}>{t.pattern}</button
                  ></td
                >
                <td>{t.invocations}</td>
                <td>{fmtMs(t.totalMs)}</td>
                <td>{fmtMs(t.p50Ms)}</td>
                <td>{fmtMs(t.p95Ms)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        </div>
      </section>
    {/if}
  {/if}
</section>

<style>

  .analytics {
    min-width: 0;
    overflow: auto;
  }

  .analytics > section {
    padding: 24px 0 0;
    margin-top: 24px;
  }

  .analytics > section h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }

  .filters input,
  .filters select,
  .breakdown select {
    height: 32px;
    background: var(--muted);
    border: 1px solid var(--border);
    padding: 0 8px;
    border-radius: 8px;
    color: var(--foreground);
    font: inherit;
  }

  .breakdown {
    display: flex;
    gap: 4px;
    align-items: center;
    margin-top: 10px;
    font-size: 12px;
    color: var(--muted-foreground);
  }

  .filters {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
  }

  .date-filters {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .date-error {
    flex-basis: 100%;
    margin: 0;
  }

  .filters label {
    display: flex;
    gap: 4px;
    align-items: center;
  }

  .chart-scroll {
    overflow-x: auto;
  }

  svg {
    height: 240px;
    display: block;
  }

  svg line {
    stroke: var(--border);
  }

  svg text {
    fill: var(--foreground);
    font-size: 11px;
  }

  /* Shares the timeline trace palette (timeline.ts eventColors). */

  rect {
    fill: var(--chart-bar, #0891b2);
  }

  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    margin: 6px 0 0;
    font-size: 11px;
    color: var(--muted-foreground);
  }

  .legend span {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    overflow-wrap: anywhere;
  }

  .sw {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 2px;
  }

  .line-p50 {
    fill: none;
    stroke: var(--success);
    stroke-width: 2;
  }

  .line-p95 {
    fill: none;
    stroke: var(--warning);
    stroke-width: 1.5;
    stroke-dasharray: 4 3;
  }

  .dot-p50 {
    fill: var(--success);
  }

  .dot-p95 {
    fill: var(--warning);
  }

  .sw-p50 {
    background: var(--success);
  }

  .sw-p95 {
    background: var(--warning);
  }

  .table-scroll {
    overflow-x: auto;
  }

  .patterns {
    width: 100%;
    min-width: 560px;
    margin-top: 10px;
    border-collapse: collapse;
  }

  .patterns th,
  .patterns td {
    padding: 4px 8px;
    font-size: 12px;
    text-align: right;
  }

  .patterns th:first-child,
  .patterns td:first-child {
    text-align: left;
  }

  .patterns thead th {
    color: var(--muted-foreground);
    font-weight: 500;
    border-bottom: 1px solid var(--border);
  }

  .patterns tr.active td {
    background: var(--accent);
  }

  .target-link {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    font-family: var(--mono);
    color: inherit;
    cursor: pointer;
    text-align: left;
  }

  .target-link:hover {
    text-decoration: underline;
  }

  .hint {
    margin: 4px 0 0;
  }

  .day-hit {
    fill: transparent;
    cursor: pointer;
  }

  .day-hit:hover {
    fill: color-mix(in srgb, var(--foreground) 8%, transparent);
  }

  .day-hit:focus-visible {
    fill: color-mix(in srgb, var(--primary) 12%, transparent);
    stroke: var(--ring);
    stroke-width: 1.5;
    outline: none;
  }

  .grid-line {
    stroke: var(--hairline);
    stroke-dasharray: 2 3;
  }

  .bar-analysis {
    fill: var(--warning);
  }

  .bar-execution {
    fill: var(--chart-bar, #0891b2);
  }

  .sw-analysis {
    background: var(--warning);
  }

  .sw-execution {
    background: var(--chart-bar, #0891b2);
  }

  .line-cache {
    fill: none;
    stroke: var(--chart-bar, #0891b2);
    stroke-width: 2;
  }

  .dot-cache {
    fill: var(--chart-bar, #0891b2);
  }
</style>
