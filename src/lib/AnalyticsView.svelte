<script lang="ts">
  import Notice from "./Notice.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import {
    analyticsDateError,
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

  let { workspace, invocations }: {
    workspace: string;
    invocations: InvocationRecord[];
  } = $props();
  let view = $state.raw(emptyAnalytics());
  let controller = $state<ReturnType<typeof createAnalytics> | null>(null);
  let chartWidth = $state(0);
  $effect(() => {
    // A workspace change discards both the filters and pending responses.
    void workspace;
    const next = createAnalytics({ invoke }, (value) => (view = value));
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
  const width = $derived(
    Math.max(640, (data?.days.length ?? 0) * 24 + 60, chartWidth),
  );
  const step = $derived((width - 70) / Math.max(1, data?.days.length ?? 1));
  const dayBars = $derived(
    (data?.days ?? []).map((day, index) => {
      let top = PLOT.baseline;
      const rects = OUTCOMES.flatMap((kind) => {
        const height = (day[kind] / max) * PLOT.height;
        if (height <= 0) return [];
        top -= height;
        return [{ kind, x: PLOT.left + index * step, y: top, height }];
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
</script>

{#snippet edgeLabels()}
  {#each edgeDays as { index, day }}
    <text x={PLOT.left + index * step} y="218" text-anchor={index === 0 ? "start" : "end"}
      >{day}</text
    >
  {/each}
{/snippet}

<section class="analytics">
  <h1>Analytics</h1>
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
    <section class="analytics-card">
      <h2>Invocations</h2>
      <p class="dim">{data.filter.start} to {data.filter.end} · Invocations without a recorded start time are excluded.</p>
      <div class="chart-scroll" bind:clientWidth={chartWidth}>
        <svg
          viewBox="0 0 {width} 240"
          style:width="{width}px"
          role="img"
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
          <text x="4" y="16">Count</text><text x="12" y="34">{max}</text><text
            x="30"
            y={PLOT.baseline}>0</text
          >
          <text x={width - 45} y="236">Date</text>
          {#each dayBars as { day, rects }}
            <g>
              <title>{`${day.day}: ${day.count} ${day.count === 1 ? "invocation" : "invocations"} (${OUTCOMES.map((kind) => `${kind} ${day[kind]}`).join(", ")})${day.day === data.today ? " (in progress)" : ""}`}</title>
              {#each rects as rect}
                <rect
                  class="bar-{rect.kind}"
                  x={rect.x}
                  y={rect.y}
                  width={Math.max(1, step - 3)}
                  height={rect.height}
                />
              {/each}
            </g>
          {/each}
          {@render edgeLabels()}
        </svg>
      </div>
      <p class="legend">
        {#each OUTCOMES as kind}
          <span><i class="sw bar-{kind}"></i>{kind}</span>
        {/each}
      </p>
    </section>
    <section class="analytics-card">
      <h2>Total duration</h2>
      <p class="dim">Total duration per day</p>
      <div class="chart-scroll" bind:clientWidth={chartWidth}>
        <svg
          viewBox="0 0 {width} 240"
          style:width="{width}px"
          role="img"
          aria-label="Total duration per day"
        >
          <title>Total duration per day</title>
          <line x1="50" y1="20" x2="50" y2={PLOT.baseline} />
          <line x1="50" y1={PLOT.baseline} x2={width - 10} y2={PLOT.baseline} />
          <text x="4" y="16">{fmtMs(maxTotal)}</text><text x="30" y={PLOT.baseline}
            >0</text
          >
          <text x={width - 45} y="236">Date</text>
          {#each data.days as day, index}
            <rect
              x={PLOT.left + index * step}
              y={PLOT.baseline - (day.totalMs / maxTotal) * PLOT.height}
              width={Math.max(1, step - 3)}
              height={(day.totalMs / maxTotal) * PLOT.height}
            >
              <title>{`${day.day}: ${fmtMs(day.totalMs)}${day.day === data.today ? " (in progress)" : ""}`}</title>
            </rect>
          {/each}
          {@render edgeLabels()}
        </svg>
      </div>
    </section>
    <section class="analytics-card">
      <h2>Duration</h2>
      <p class="dim">Per-day invocation duration percentiles</p>
      <div class="chart-scroll" bind:clientWidth={chartWidth}>
        <svg
          viewBox="0 0 {width} 240"
          style:width="{width}px"
          role="img"
          aria-label="Per-day invocation duration percentiles"
        >
          <title>Per-day invocation duration percentiles</title>
          <line x1="50" y1="20" x2="50" y2={PLOT.baseline} />
          <line x1="50" y1={PLOT.baseline} x2={width - 10} y2={PLOT.baseline} />
          <text x="4" y="16">{fmtMs(maxP95)}</text><text x="30" y={PLOT.baseline}>0</text>
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
              >
                <title>{`${day.day}: p95 ${fmtMs(day.p95Ms)}`}</title>
              </circle>
              <circle
                class="dot-p50"
                cx={PLOT.left + index * step + step / 2}
                cy={PLOT.baseline - (day.p50Ms / maxP95) * PLOT.height}
                r="2.5"
              >
                <title>{`${day.day}: p50 ${fmtMs(day.p50Ms)}, p95 ${fmtMs(day.p95Ms)}`}</title>
              </circle>
            {/if}
          {/each}
          {@render edgeLabels()}
        </svg>
      </div>
      <p class="legend">
        <span><i class="sw sw-p50"></i>p50</span>
        <span><i class="sw sw-p95"></i>p95</span>
      </p>
    </section>
    {#if data.patterns.length}
      <section class="analytics-card">
        <h2>Target patterns</h2>
        <p class="dim">Complete pattern lists by total duration</p>
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
      </section>
    {/if}
  {/if}
</section>

<style>

  .analytics {
    min-width: 0;
    padding: 28px;
    overflow: auto;
  }

  .analytics h1 {
    margin: 0 0 24px;
    font-size: 22px;
    letter-spacing: -0.7px;
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .analytics-card {
    background: var(--muted);
    border-radius: var(--radius-lg);
    padding: 22px;
    margin-top: 20px;
  }

  .analytics-card h2 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }

  .filters input,
  .filters select {
    height: 32px;
    background: var(--muted);
    border: 1px solid var(--border);
    padding: 0 8px;
    border-radius: 8px;
    color: var(--foreground);
    font: inherit;
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
    fill: #99d4c6;
  }

  .bar-ok {
    fill: #a7d6a9;
    background: #a7d6a9;
  }

  .bar-failed {
    fill: #ffdda6;
    background: #ffdda6;
  }

  .bar-interrupted {
    fill: #c5afd4;
    background: #c5afd4;
  }

  .bar-unknown {
    fill: #b2b2b2;
    background: #b2b2b2;
  }

  .legend {
    display: flex;
    gap: 14px;
    margin: 6px 0 0;
    font-size: 11px;
    color: var(--muted-foreground);
  }

  .legend span {
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }

  .sw {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 2px;
  }

  .line-p50 {
    fill: none;
    stroke: #80c784;
    stroke-width: 2;
  }

  .line-p95 {
    fill: none;
    stroke: #c5afd4;
    stroke-width: 1.5;
    stroke-dasharray: 4 3;
  }

  .dot-p50 {
    fill: #80c784;
  }

  .dot-p95 {
    fill: #c5afd4;
  }

  .sw-p50 {
    background: #80c784;
  }

  .sw-p95 {
    background: #c5afd4;
  }

  .patterns {
    width: 100%;
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
</style>
