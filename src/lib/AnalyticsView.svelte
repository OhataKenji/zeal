<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import {
    createAnalytics,
    emptyAnalytics,
    fmtMs,
    segments,
    type AnalyticsFilter,
  } from "./analytics";
  import type { RunRecord } from "./types";

  let { workspace, runs }: { workspace: string; runs: RunRecord[] } = $props();
  let view = $state.raw(emptyAnalytics());
  let controller = $state<ReturnType<typeof createAnalytics> | null>(null);
  $effect(() => {
    // A workspace change discards both the filters and pending responses.
    void workspace;
    const next = createAnalytics({ invoke }, (value) => (view = value));
    controller = next;
    return next.dispose;
  });
  $effect(() => {
    void runs;
    if (controller) void controller.load();
  });
  const data = $derived(view.data);
  const max = $derived(Math.max(1, ...(data?.days.map((d) => d.count) ?? [])));
  const maxTotal = $derived(
    Math.max(1, ...(data?.days.map((d) => d.totalMs) ?? [])),
  );
  const maxP95 = $derived(
    Math.max(1, ...(data?.days.map((d) => d.p95Ms) ?? [])),
  );
  const width = $derived(Math.max(640, (data?.days.length ?? 0) * 24 + 60));
  const step = $derived((width - 70) / Math.max(1, data?.days.length ?? 1));
  function filter(patch: Partial<AnalyticsFilter>) {
    void controller?.load({ ...view.filter, ...patch });
  }
</script>

<section class="analytics">
  <h1>Analytics</h1>
  <div class="filters">
    <label
      >Start date <input
        type="date"
        aria-label="Start date"
        value={view.filter.start ?? ""}
        onchange={(e) => filter({ start: e.currentTarget.value || null })}
      /></label
    >
    <label
      >End date <input
        type="date"
        aria-label="End date"
        value={view.filter.end ?? ""}
        onchange={(e) => filter({ end: e.currentTarget.value || null })}
      /></label
    >
    <label
      >Command <select
        aria-label="Command"
        value={view.filter.cmd ?? ""}
        onchange={(e) =>
          filter({
            cmd: (e.currentTarget.value as AnalyticsFilter["cmd"]) || null,
          })}
      >
        <option value="">All</option><option>build</option><option
          >test</option
        >
      </select></label
    >
    <label
      >Result <select
        aria-label="Result"
        value={view.filter.outcome ?? ""}
        onchange={(e) =>
          filter({
            outcome:
              (e.currentTarget.value as AnalyticsFilter["outcome"]) || null,
          })}
      >
        <option value="">All</option><option>ok</option><option
          >failed</option
        ><option>interrupted</option>
      </select></label
    >
    <label
      >Target <input
        type="text"
        aria-label="Target"
        placeholder="//app:server"
        value={view.filter.target ?? ""}
        onchange={(e) =>
          filter({ target: e.currentTarget.value || null })}
      /></label
    >
    <button type="button" class="button" onclick={() => controller?.load()}
      >Refresh</button
    >
  </div>
  {#if view.loading}<p role="status">Loading…</p>{/if}
  {#if view.error}<p class="err" role="alert">
      Unable to load analytics: {view.error}
    </p>{/if}
  {#if data}
    <section class="analytics-card">
      <h2>Runs</h2>
      <p class="dim">{data.filter.start} to {data.filter.end}</p>
      <div class="chart-scroll">
        <svg
          viewBox="0 0 {width} 240"
          style:width="{width}px"
          role="img"
          aria-label="Runs per day"
          aria-describedby="analytics-daily-counts"
        >
          <title>Runs per day</title>
          <desc id="analytics-daily-counts">
            {data.days
              .map(
                ({ day, count }) =>
                  `${day}: ${count}${day === data.today ? " (in progress)" : ""}`,
              )
              .join("; ")}
          </desc>
          <line x1="50" y1="20" x2="50" y2="200" />
          <line x1="50" y1="200" x2={width - 10} y2="200" />
          <text x="4" y="16">Count</text><text x="12" y="34">{max}</text><text
            x="30"
            y="200">0</text
          >
          <text x={width - 45} y="236">Date</text>
          {#each data.days as day, index}
            {@const h = (day.count / max) * 160}
            {@const okH = day.count ? (day.ok / day.count) * h : 0}
            {@const failedH = day.count ? (day.failed / day.count) * h : 0}
            <g>
              <title>{`${day.day}: ${day.count} ${day.count === 1 ? "run" : "runs"} (ok ${day.ok}, failed ${day.failed}, interrupted ${day.interrupted})${day.day === data.today ? " (in progress)" : ""}`}</title>
              {#if okH > 0}
                <rect
                  class="bar-ok"
                  x={55 + index * step}
                  y={200 - okH}
                  width={Math.max(1, step - 3)}
                  height={okH}
                />
              {/if}
              {#if failedH > 0}
                <rect
                  class="bar-failed"
                  x={55 + index * step}
                  y={200 - okH - failedH}
                  width={Math.max(1, step - 3)}
                  height={failedH}
                />
              {/if}
              {#if h - okH - failedH > 0}
                <rect
                  class="bar-interrupted"
                  x={55 + index * step}
                  y={200 - h}
                  width={Math.max(1, step - 3)}
                  height={h - okH - failedH}
                />
              {/if}
            </g>
            {#if index === 0 || index === data.days.length - 1}
              <text
                x={55 + index * step}
                y="218"
                text-anchor={index === 0 ? "start" : "end"}>{day.day}</text
              >
            {/if}
          {/each}
        </svg>
      </div>
      <p class="legend">
        <span><i class="sw bar-ok"></i>ok</span>
        <span><i class="sw bar-failed"></i>failed</span>
        <span><i class="sw bar-interrupted"></i>interrupted</span>
      </p>
    </section>
    <section class="analytics-card">
      <h2>Wait time</h2>
      <p class="dim">Total run time per day</p>
      <div class="chart-scroll">
        <svg
          viewBox="0 0 {width} 240"
          style:width="{width}px"
          role="img"
          aria-label="Total run time per day"
        >
          <title>Total run time per day</title>
          <line x1="50" y1="20" x2="50" y2="200" />
          <line x1="50" y1="200" x2={width - 10} y2="200" />
          <text x="4" y="16">{fmtMs(maxTotal)}</text><text x="30" y="200"
            >0</text
          >
          <text x={width - 45} y="236">Date</text>
          {#each data.days as day, index}
            <rect
              x={55 + index * step}
              y={200 - (day.totalMs / maxTotal) * 160}
              width={Math.max(1, step - 3)}
              height={(day.totalMs / maxTotal) * 160}
            >
              <title>{`${day.day}: ${fmtMs(day.totalMs)}${day.day === data.today ? " (in progress)" : ""}`}</title>
            </rect>
            {#if index === 0 || index === data.days.length - 1}
              <text
                x={55 + index * step}
                y="218"
                text-anchor={index === 0 ? "start" : "end"}>{day.day}</text
              >
            {/if}
          {/each}
        </svg>
      </div>
    </section>
    <section class="analytics-card">
      <h2>Duration</h2>
      <p class="dim">Per-day run duration percentiles</p>
      <div class="chart-scroll">
        <svg
          viewBox="0 0 {width} 240"
          style:width="{width}px"
          role="img"
          aria-label="Run duration percentiles per day"
        >
          <title>Run duration percentiles per day</title>
          <line x1="50" y1="20" x2="50" y2="200" />
          <line x1="50" y1="200" x2={width - 10} y2="200" />
          <text x="4" y="16">{fmtMs(maxP95)}</text><text x="30" y="200">0</text>
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
                cx={55 + index * step + step / 2}
                cy={200 - (day.p95Ms / maxP95) * 160}
                r="2.5"
              >
                <title>{`${day.day}: p95 ${fmtMs(day.p95Ms)}`}</title>
              </circle>
              <circle
                class="dot-p50"
                cx={55 + index * step + step / 2}
                cy={200 - (day.p50Ms / maxP95) * 160}
                r="2.5"
              >
                <title>{`${day.day}: p50 ${fmtMs(day.p50Ms)}, p95 ${fmtMs(day.p95Ms)}`}</title>
              </circle>
            {/if}
            {#if index === 0 || index === data.days.length - 1}
              <text
                x={55 + index * step}
                y="218"
                text-anchor={index === 0 ? "start" : "end"}>{day.day}</text
              >
            {/if}
          {/each}
        </svg>
      </div>
      <p class="legend">
        <span><i class="sw sw-p50"></i>p50</span>
        <span><i class="sw sw-p95"></i>p95</span>
      </p>
    </section>
    {#if data.targets.length}
      <section class="analytics-card">
        <h2>Targets</h2>
        <p class="dim">By total wait time</p>
        <table class="targets">
          <thead
            ><tr
              ><th>Target</th><th>Runs</th><th>Total</th><th>p50</th><th
                >p95</th
              ></tr
            ></thead
          >
          <tbody>
            {#each data.targets as t}
              <tr class:active={view.filter.target === t.target}>
                <td
                  ><button
                    type="button"
                    class="target-link"
                    onclick={() =>
                      filter({
                        target:
                          view.filter.target === t.target ? null : t.target,
                      })}>{t.target}</button
                  ></td
                >
                <td>{t.runs}</td>
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
    background: var(--muted);
    border: 1px solid var(--border);
    padding: 8px;
    border-radius: 8px;
    color: var(--foreground);
    font: inherit;
  }
  .filters {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
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
  }
  .bar-failed {
    fill: #ffdda6;
  }
  .bar-interrupted {
    fill: #c5afd4;
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
  .targets {
    width: 100%;
    margin-top: 10px;
    border-collapse: collapse;
  }
  .targets th,
  .targets td {
    padding: 4px 8px;
    font-size: 12px;
    text-align: right;
  }
  .targets th:first-child,
  .targets td:first-child {
    text-align: left;
  }
  .targets thead th {
    color: var(--muted-foreground);
    font-weight: 500;
    border-bottom: 1px solid var(--border);
  }
  .targets tr.active td {
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
