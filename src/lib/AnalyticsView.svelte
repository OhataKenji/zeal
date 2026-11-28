<script lang="ts">
  import { untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
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
  import Notice from "./Notice.svelte";
  import type { InvocationRecord } from "./types";

  const OUTCOMES = ["ok", "failed", "interrupted", "unknown"] as const;
  let {
    workspace,
    invocations,
    onselectday,
  }: {
    workspace: string;
    invocations: InvocationRecord[];
    onselectday?: (day: string) => void;
  } = $props();
  let view = $state(emptyAnalytics());
  let controller: ReturnType<typeof createAnalytics> | null = null;
  let breakdown = $state<"result" | "workspace">("result");
  let chartWidth = $state(0);

  $effect(() => {
    const next = createAnalytics(workspace, { invoke }, (updated) => (view = updated));
    controller = next;
    void next.load();
    return () => {
      next.dispose();
      if (controller === next) controller = null;
    };
  });

  // Reload analytics when persisted history changes, without reloading on mount.
  let seen = untrack(() => invocations);
  $effect(() => {
    if (invocations === seen) return;
    seen = invocations;
    void controller?.load();
  });

  const data = $derived(view.data);
  const dateError = $derived(analyticsDateError(view.filter));
  const max = $derived(Math.max(1, ...(data?.days.map((d) => d.count) ?? [0])));
  const maxTotal = $derived(Math.max(1, ...(data?.days.map((d) => d.totalMs) ?? [0])));
  const maxP95 = $derived(Math.max(1, ...(data?.days.map((d) => d.p95Ms) ?? [0])));
  const maxPhase = $derived(
    Math.max(1, ...(data?.days.map((d) => d.analysisMs + d.executionMs) ?? [0])),
  );
  const hasPhaseData = $derived(data?.days.some((d) => d.phaseTimed > 0) ?? false);
  const hasCacheData = $derived(data?.days.some((d) => d.actionsTotal > 0) ?? false);
  const width = $derived(Math.max(640, (data?.days.length ?? 0) * 24 + 60, chartWidth));
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
  function onDayKey(event: KeyboardEvent, day: string) {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onselectday?.(day);
    }
  }
  function dayCountTip(day: NonNullable<typeof data>["days"][number], index: number): string {
    const rects = dayBars[index]?.rects ?? [];
    const parts = rects.length > 0 ? ` (${rects.map((r) => `${r.label} ${r.count}`).join(", ")})` : "";
    const progress = day.day === data?.today ? " (in progress)" : "";
    const hint = day.count > 0 ? " — open in History" : "";
    return `${day.day}: ${day.count} ${day.count === 1 ? "invocation" : "invocations"}${parts}${progress}${hint}`;
  }
  function dayTotalTip(day: NonNullable<typeof data>["days"][number]): string {
    const progress = day.day === data?.today ? " (in progress)" : "";
    const hint = day.count > 0 ? " — open in History" : "";
    return `${day.day}: ${fmtMs(day.totalMs)}${progress}${hint}`;
  }
  function dayDurationTip(day: NonNullable<typeof data>["days"][number]): string {
    const hint = day.count > 0 ? " — open in History" : "";
    return day.timed
      ? `${day.day}: p50 ${fmtMs(day.p50Ms)}, p95 ${fmtMs(day.p95Ms)}${hint}`
      : `${day.day}: no timed invocations${hint}`;
  }
  function dayPhaseTip(day: NonNullable<typeof data>["days"][number]): string {
    const hint = day.count > 0 ? " — open in History" : "";
    return day.phaseTimed > 0
      ? `${day.day}: avg analysis ${fmtMs(day.analysisMs)}, avg execution ${fmtMs(day.executionMs)}${hint}`
      : `${day.day}: no phase metrics${hint}`;
  }
  function dayCacheTip(day: NonNullable<typeof data>["days"][number]): string {
    const rate = cacheHitRate(day.cacheHits, day.actionsTotal);
    const hint = day.count > 0 ? " — open in History" : "";
    return rate !== null
      ? `${day.day}: ${rate}% cache hits (${day.cacheHits}/${day.actionsTotal})${hint}`
      : `${day.day}: no cache metrics${hint}`;
  }
</script>

{#snippet edgeLabels()}
  {#each edgeDays as { index, day }}
    <text
      x={index === 0 ? PLOT.left : PLOT.left + index * step + Math.max(1, step - 3)}
      y="218"
      text-anchor={index === 0 ? "start" : "end"}>{day}</text
    >
  {/each}
{/snippet}

{#snippet dayHits(describe: (day: NonNullable<typeof data>["days"][number], index: number) => string)}
  {#each data?.days ?? [] as day, index}
    {#if day.count > 0}
      {@const tip = describe(day, index)}
      <rect
        class="day-hit"
        x={PLOT.left + index * step - 1.5}
        y="20"
        width={Math.max(4, step)}
        height={PLOT.baseline - 20}
        role="button"
        tabindex="0"
        aria-label={tip}
        onclick={() => onselectday?.(day.day)}
        onkeydown={(e) => onDayKey(e, day.day)}
      >
        <title>{tip}</title>
      </rect>
    {/if}
  {/each}
{/snippet}

<section class="analytics">
  <div class="view-head">
    <h1 class="view-title">Analytics</h1>
    <p class="hint">Click any day to inspect its slowest invocations in History.</p>
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
        onchange={(e) => filter({ workspaceId: e.currentTarget.value || null })}
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
            outcome: (e.currentTarget.value as AnalyticsFilter["outcome"]) || null,
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
        onchange={(e) => filter({ pattern: e.currentTarget.value || null })}
      /></label
    >
    <button type="button" class="button" onclick={() => controller?.load()}>Refresh</button>
  </div>
  {#if view.loading}<p role="status">Loading…</p>{/if}
  {#if view.error}<Notice tone="error">Unable to load analytics: {view.error}</Notice>{/if}
  {#if data}
    <section>
      <div class="chart-head">
        <h2>Invocations per day</h2>
        <label class="breakdown"
          >Breakdown <select aria-label="Breakdown" bind:value={breakdown}>
            <option value="result">Result</option>
            <option value="workspace">Workspace</option>
          </select></label
        >
      </div>
      <div class="chart-scroll" bind:clientWidth={chartWidth}>
        <svg
          viewBox="0 0 {width} 240"
          style:width="{width}px"
          role="group"
          aria-label="Invocations per day"
          aria-describedby="analytics-daily-counts"
        >
          <title>Invocations per day</title>
          <desc id="analytics-daily-counts">
            {data.days
              .map(
                ({ day, count }) =>
                  `${day}: ${count}${day === data.today ? " (in progress)" : ""}`,
              )
              .join("; ")}
          </desc>
          <line x1="50" y1="20" x2="50" y2={PLOT.baseline} />
          <line x1="50" y1={PLOT.baseline} x2={width - 10} y2={PLOT.baseline} />
          <text x="44" y="16" text-anchor="end">Count</text>
          <text x="44" y={PLOT.baseline - PLOT.height + 4} text-anchor="end">{max}</text>
          <text x="44" y={PLOT.baseline} text-anchor="end">0</text>
          <text x={width - 45} y="236">Date</text>
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
          {@render dayHits(dayCountTip)}
          {@render edgeLabels()}
        </svg>
      </div>
      {#if series.length > 0}
        <p class="legend">
          {#each series as item (item.key)}
            <span><i class="sw" style:background={item.color}></i>{item.label}</span>
          {/each}
        </p>
      {/if}
    </section>
    <section>
      <h2>Total duration per day</h2>
      <div class="chart-scroll" bind:clientWidth={chartWidth}>
        <svg
          viewBox="0 0 {width} 240"
          style:width="{width}px"
          role="group"
          aria-label="Total duration per day"
        >
          <title>Total duration per day</title>
          <line x1="50" y1="20" x2="50" y2={PLOT.baseline} />
          <line x1="50" y1={PLOT.baseline} x2={width - 10} y2={PLOT.baseline} />
          <text x="44" y={PLOT.baseline - PLOT.height + 4} text-anchor="end">{fmtMs(maxTotal)}</text>
          <text x="44" y={PLOT.baseline} text-anchor="end">0</text>
          <text x={width - 45} y="236">Date</text>
          {#each data.days as day, index}
            <rect
              x={PLOT.left + index * step}
              y={PLOT.baseline - (day.totalMs / maxTotal) * PLOT.height}
              width={Math.max(1, step - 3)}
              height={(day.totalMs / maxTotal) * PLOT.height}
            />
          {/each}
          {@render dayHits(dayTotalTip)}
          {@render edgeLabels()}
        </svg>
      </div>
    </section>
    <section>
      <h2>Duration per invocation</h2>
      <p class="dim">Daily p50 and p95</p>
      <div class="chart-scroll" bind:clientWidth={chartWidth}>
        <svg
          viewBox="0 0 {width} 240"
          style:width="{width}px"
          role="group"
          aria-label="Per-day invocation duration percentiles"
        >
          <title>Per-day invocation duration percentiles</title>
          <line x1="50" y1="20" x2="50" y2={PLOT.baseline} />
          <line x1="50" y1={PLOT.baseline} x2={width - 10} y2={PLOT.baseline} />
          <text x="44" y={PLOT.baseline - PLOT.height + 4} text-anchor="end">{fmtMs(maxP95)}</text>
          <text x="44" y={PLOT.baseline} text-anchor="end">0</text>
          <text x={width - 45} y="236">Date</text>
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
          {@render dayHits(dayDurationTip)}
          {@render edgeLabels()}
        </svg>
      </div>
      <p class="legend">
        <span><i class="sw sw-p50"></i>p50</span><span><i class="sw sw-p95"></i>p95</span>
      </p>
    </section>
    {#if hasPhaseData}
      <section>
        <h2>Phase duration</h2>
        <p class="dim">Average analysis vs execution phase time per invocation</p>
        <div class="chart-scroll" bind:clientWidth={chartWidth}>
          <svg
            viewBox="0 0 {width} 240"
            style:width="{width}px"
            role="group"
            aria-label="Phase duration per day"
          >
            <title>Phase duration per day</title>
            <line x1="50" y1="20" x2="50" y2={PLOT.baseline} />
            <line x1="50" y1={PLOT.baseline} x2={width - 10} y2={PLOT.baseline} />
            <text x="44" y={PLOT.baseline - PLOT.height + 4} text-anchor="end">{fmtMs(maxPhase)}</text>
            <text x="44" y={PLOT.baseline} text-anchor="end">0</text>
            <text x={width - 45} y="236">Date</text>
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
            {@render dayHits(dayPhaseTip)}
            {@render edgeLabels()}
          </svg>
        </div>
        <p class="legend">
          <span><i class="sw sw-analysis"></i>Analysis</span>
          <span><i class="sw sw-execution"></i>Execution</span>
        </p>
      </section>
    {/if}
    {#if hasCacheData}
      <section>
        <h2>Cache hit rate</h2>
        <p class="dim">Daily action cache hit percentage (local + remote/disk)</p>
        <div class="chart-scroll" bind:clientWidth={chartWidth}>
          <svg
            viewBox="0 0 {width} 240"
            style:width="{width}px"
            role="group"
            aria-label="Cache hit rate per day"
          >
            <title>Cache hit rate per day</title>
            <line x1="50" y1="20" x2="50" y2={PLOT.baseline} />
            <line x1="50" y1={PLOT.baseline} x2={width - 10} y2={PLOT.baseline} />
            <line
              class="grid-line"
              x1="50"
              y1={PLOT.baseline - PLOT.height / 2}
              x2={width - 10}
              y2={PLOT.baseline - PLOT.height / 2}
            />
            <text x="44" y={PLOT.baseline - PLOT.height + 4} text-anchor="end">100%</text>
            <text x="44" y={PLOT.baseline - PLOT.height / 2 + 4} text-anchor="end">50%</text>
            <text x="44" y={PLOT.baseline} text-anchor="end">0%</text>
            <text x={width - 45} y="236">Date</text>
            {#each segments(
              data.days.map((d) => cacheHitRate(d.cacheHits, d.actionsTotal)),
              100,
              step,
            ) as points}
              <polyline class="line-cache" {points} />
            {/each}
            {#each data.days as day, index}
              {@const rate = cacheHitRate(day.cacheHits, day.actionsTotal)}
              {#if rate !== null}
                <circle
                  class="dot-cache"
                  cx={PLOT.left + index * step + step / 2}
                  cy={PLOT.baseline - (rate / 100) * PLOT.height}
                  r="2.5"
                />
              {/if}
            {/each}
            {@render dayHits(dayCacheTip)}
            {@render edgeLabels()}
          </svg>
        </div>
      </section>
    {/if}
    <section>
      <h2>Duration by target patterns</h2>
      {#if data.patterns.length === 0}
        <p class="dim">No timed invocations match these filters.</p>
      {:else}
        <div class="table-scroll">
          <table class="patterns">
            <thead>
              <tr><th>Target patterns</th><th>Count</th><th>Total</th><th>p50</th><th>p95</th></tr>
            </thead>
            <tbody>
              {#each data.patterns as row (row.pattern)}
                <tr class:active={view.filter.pattern === row.pattern}>
                  <td
                    ><button
                      type="button"
                      class="target-link"
                      onclick={() =>
                        filter({
                          pattern: view.filter.pattern === row.pattern ? null : row.pattern,
                        })}
                    >
                      {row.pattern}
                    </button></td
                  >
                  <td>{row.invocations}</td>
                  <td>{fmtMs(row.totalMs)}</td>
                  <td>{fmtMs(row.p50Ms)}</td>
                  <td>{fmtMs(row.p95Ms)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </section>
  {/if}
</section>

<style>
  .analytics {
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .analytics > section {
    border: none;
    border-radius: 0;
    padding: 0;
    background: transparent;
  }

  .analytics > section h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }

  .hint {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--muted-foreground);
  }

  .filters {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    align-items: flex-end;
  }

  .date-filters {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 10px;
    align-items: flex-end;
  }

  .date-error {
    flex-basis: 100%;
    margin: 0;
  }

  .filters label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
  }

  .filters input,
  .filters select,
  .breakdown select {
    height: 32px;
    background: var(--background);
    color: var(--foreground);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 4px 8px;
  }

  .chart-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }

  .breakdown {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }

  .chart-scroll {
    overflow-x: auto;
  }

  .table-scroll {
    overflow-x: auto;
  }

  svg {
    display: block;
    min-width: 100%;
    height: auto;
  }

  line {
    stroke: var(--border);
  }

  text {
    fill: var(--foreground);
    font-size: 11px;
  }

  rect {
    fill: var(--chart-bar, #0891b2);
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

  .sw-p50 {
    background: var(--success);
  }

  .sw-p95 {
    background: var(--warning);
  }

  .patterns {
    width: 100%;
    min-width: 560px;
    border-collapse: collapse;
    margin-top: 10px;
  }

  .patterns th,
  .patterns td {
    padding: 4px 8px;
    font-size: 12px;
    text-align: right;
  }

  .patterns thead th {
    color: var(--muted-foreground);
    font-weight: 500;
    border-bottom: 1px solid var(--border);
  }

  .patterns tr.active td {
    background: var(--accent);
  }

  .patterns td {
    font-variant-numeric: tabular-nums;
  }

  .patterns th:first-child,
  .patterns td:first-child {
    text-align: left;
  }

  .target-link {
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    font-family: var(--mono);
    padding: 0;
    cursor: pointer;
    text-align: left;
  }

  .target-link:hover {
    text-decoration: underline;
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
