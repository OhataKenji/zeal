<script lang="ts">
  import type { Bep, Count, Section } from "../types";
  import { sectionData, sectionMessage, sectionNoticeTone } from "../invocation";
  import Notice from "../Notice.svelte";

  let { section, loading }: {
    section: Section<Bep> | undefined;
    loading: boolean;
  } = $props();

  const bep = $derived(sectionData(section));
  const stats = $derived(bep?.cacheStats);
  const local = $derived(stats?.localActionCache);

  type DonutRow = Count & { color?: string };
  interface DonutSlice {
    name: string;
    count: number;
    color: string;
    start: number;
    end: number;
    pct: number;
  }

  // Theme tokens only; the folded tail stays inside the percentage total.
  const PALETTE = [
    "var(--info)",
    "var(--brand-teal)",
    "var(--success)",
    "var(--warning)",
    "var(--test-color)",
  ];
  const OTHER_COLOR = "var(--muted-foreground)";
  const NEUTRAL = "var(--accent)";

  function donutSlices(rows: DonutRow[], fixedOrder: boolean): DonutSlice[] {
    const sorted = fixedOrder ? rows : [...rows].sort((a, b) => b.count - a.count);
    const head = sorted.slice(0, 5);
    const tail = sorted.slice(5);
    const entries =
      tail.length > 0
        ? [
            ...head,
            { name: "Other", count: tail.reduce((sum, row) => sum + row.count, 0) },
          ]
        : head;
    const total = entries.reduce((sum, row) => sum + row.count, 0);
    let start = 0;
    return entries.map((row, index) => {
      const end = total > 0 ? start + (row.count / total) * 100 : start;
      const slice: DonutSlice = {
        name: row.name,
        count: row.count,
        color:
          row.color ??
          (tail.length > 0 && index === entries.length - 1
            ? OTHER_COLOR
            : PALETTE[index % PALETTE.length]),
        start,
        end,
        pct: total > 0 ? Math.round((row.count / total) * 100) : 0,
      };
      start = end;
      return slice;
    });
  }

  function donutBackground(slices: DonutSlice[]): string {
    return slices.some((slice) => slice.end > slice.start)
      ? `conic-gradient(${slices
          .map((slice) => `${slice.color} ${slice.start}% ${slice.end}%`)
          .join(", ")})`
      : NEUTRAL;
  }

  const localRows = $derived<DonutRow[]>(
    local
      ? [
          {
            name: local.hits === 1 ? "hit" : "hits",
            count: local.hits,
            color: "var(--success)",
          },
          {
            name: local.misses === 1 ? "miss" : "misses",
            count: local.misses,
            color: "var(--destructive)",
          },
        ]
      : [],
  );
</script>

<div class="cache-report">
  <h3 class="panel-subheading">Cache stats</h3>
  {#if section && section.status !== "ok"}
    <Notice tone={sectionNoticeTone(section)}>{sectionMessage(section)}</Notice>
  {/if}
  {#if bep}
    {#if stats}
      <div class="cache-sections">
        {#if local}
          {@render chart(
            "Local action cache",
            "Lookups in Bazel’s local action cache",
            localRows,
            true,
            "No lookups",
          )}
        {/if}
        {#if stats.runnerTypes.length}
          {@render chart(
            "Runner types",
            "Number of actions with each runner type",
            stats.runnerTypes,
          )}
        {/if}
        {#if stats.actionTypes.length}
          {@render chart(
            "Action types",
            "Number of recorded actions by mnemonic",
            stats.actionTypes,
          )}
        {/if}
        {#if local?.missReasons.length}
          {@render chart(
            "Local invalidation reasons",
            "Reasons for missing Bazel’s local action cache",
            local.missReasons,
          )}
        {/if}
      </div>
      <p class="cache-footnote">
        {#if !local}Local action cache was not recorded.{" "}{/if}Remote AC, CAS, and transfer
        statistics are not recorded in Bazel build events.
      </p>
    {:else}
      <p class="panel-empty">No cache statistics recorded in the build events.</p>
    {/if}
  {:else if loading}
    <p class="panel-empty">Loading cache statistics…</p>
  {/if}
</div>

{#snippet chart(
  title: string,
  subtitle: string,
  rows: DonutRow[],
  fixedOrder = false,
  emptyNote = "",
)}
  {@const slices = donutSlices(rows, fixedOrder)}
  {@const total = rows.reduce((sum, row) => sum + row.count, 0)}
  <section class="cache-section">
    <h3 class="cache-title">{title}</h3>
    <p class="cache-subtitle">{subtitle}</p>
    <div class="cache-chart">
      <div
        class="cache-donut"
        style:background={donutBackground(slices)}
        aria-hidden="true"
      ></div>
      <ul class="cache-legend">
        {#each slices as slice}
          <li class="cache-legend-item">
            <span class="cache-swatch" style:background={slice.color} aria-hidden="true"></span
            ><span class="cache-legend-text"
              ><strong>{slice.count}</strong> {slice.name}{total > 0
                ? ` (${slice.pct}%)`
                : ""}</span
            >
          </li>
        {/each}
      </ul>
    </div>
    {#if total === 0 && emptyNote}
      <p class="cache-note">{emptyNote}</p>
    {/if}
  </section>
{/snippet}
