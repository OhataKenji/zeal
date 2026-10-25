<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import {
    createAnalytics,
    emptyAnalytics,
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
  const width = $derived(Math.max(640, (data?.days.length ?? 0) * 24 + 60));
  function filter(patch: Partial<AnalyticsFilter>) {
    void controller?.load({ ...view.filter, ...patch });
  }
</script>

<section class="analytics">
  <h1>Analytics</h1>
  <p class="panel-empty">Zeal-launched build and test runs.</p>
  <div class="filters">
    <label
      >Start date <input
        type="date"
        aria-label="Start date"
        value={view.filter.start ?? ""}
        onchange={(e) => filter({ start: e.currentTarget.value })}
      /></label
    >
    <label
      >End date <input
        type="date"
        aria-label="End date"
        value={view.filter.end ?? ""}
        onchange={(e) => filter({ end: e.currentTarget.value })}
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
            {@const step = (width - 70) / data.days.length}
            <rect
              rx="5"
              x={55 + index * step}
              y={200 - (day.count / max) * 160}
              width={Math.max(1, step - 3)}
              height={(day.count / max) * 160}
            >
              <title>{`${day.day}: ${day.count} ${day.count === 1 ? "run" : "runs"}${day.day === data.today ? " (in progress)" : ""}`}</title>
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
  rect {
    fill: var(--chart-bar);
  }
</style>
