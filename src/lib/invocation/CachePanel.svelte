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
  const total = $derived(local ? local.hits + local.misses : 0);
  const pct = $derived(total > 0 && local ? Math.round((local.hits / total) * 100) : 0);
  const VISIBLE_ROWS = 10;
</script>

<div class="cache-report">
  {#if section && section.status !== "ok"}
    <Notice tone={sectionNoticeTone(section)}>{sectionMessage(section)}</Notice>
  {/if}
  {#if bep}
    {#if stats}
      {#if local}
        <section class="cache-section">
          <h3 class="panel-subheading">Local action cache</h3>
          <div class="cache-local-figures">
            {#if total > 0}
              <p class="cache-rate">
                <span class="cache-rate-value mono">{pct}%</span>
                <span class="cache-rate-label">hit rate</span>
              </p>
            {:else}
              <p class="cache-rate-empty">No lookups</p>
            {/if}
            <p class="cache-local-counts">
              Hits <span class="mono">{local.hits}</span> · Misses
              <span class="mono">{local.misses}</span>
            </p>
          </div>
        </section>
      {/if}
      {#if stats.runnerTypes.length}
        {@render breakdown("Runner types", stats.runnerTypes)}
      {/if}
      {#if stats.actionTypes.length}
        {@render breakdown("Action types", stats.actionTypes)}
      {/if}
      {#if local?.missReasons.length}
        {@render breakdown("Local invalidation reasons", local.missReasons)}
      {/if}
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

{#snippet breakdown(title: string, rows: Count[])}
  {@const visible = rows.slice(0, VISIBLE_ROWS)}
  {@const max = Math.max(1, ...visible.map((row) => row.count))}
  <section class="cache-section">
    <h3 class="panel-subheading">{title}</h3>
    <ul class="cache-rows">
      {#each visible as row}
        <li class="cache-row">
          <span class="cache-row-name">{row.name}</span>
          <span class="cache-row-track" aria-hidden="true"
            ><span class="cache-row-fill" style:width="{(row.count / max) * 100}%"></span
            ></span>
          <span class="cache-row-count mono">{row.count}</span>
        </li>
      {/each}
    </ul>
    {#if rows.length > VISIBLE_ROWS}
      <p class="cache-note">Showing {VISIBLE_ROWS} of {rows.length}</p>
    {/if}
  </section>
{/snippet}
