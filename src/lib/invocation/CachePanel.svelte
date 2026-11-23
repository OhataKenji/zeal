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

{#snippet breakdown(title: string, rows: Count[])}
  <section class="cache-card">
    <h3 class="cache-card-title">{title}</h3>
    <dl class="cache-rows">
      {#each rows.slice(0, VISIBLE_ROWS) as row}
        <div class="cache-row">
          <dt>{row.name}</dt>
          <dd class="mono">{row.count}</dd>
        </div>
      {/each}
    </dl>
    {#if rows.length > VISIBLE_ROWS}
      <p class="cache-card-note">Showing {VISIBLE_ROWS} of {rows.length}</p>
    {/if}
  </section>
{/snippet}

<h3 class="panel-subheading">Cache stats</h3>
{#if section && section.status !== "ok"}
  <Notice tone={sectionNoticeTone(section)}>{sectionMessage(section)}</Notice>
{/if}
{#if bep}
  {#if stats}
    <p class="cache-remote-note">
      Remote AC, CAS, and transfer statistics are not recorded in Bazel build events.
    </p>
    <div class="cache-grid">
      {#if local}
        <section class="cache-card">
          <h3 class="cache-card-title">Local action cache</h3>
          <div class="cache-ring-wrap">
            <div class="cache-ring" style="--pct:{pct}" aria-hidden="true"></div>
            <span class="cache-ring-value">{total > 0 ? `${pct}%` : "—"}</span>
          </div>
          <p class="cache-card-note">
            Hits {local.hits} · Misses {local.misses}{total > 0 ? ` · ${pct}% hit rate` : ""}
          </p>
        </section>
      {:else}
        <section class="cache-card">
          <h3 class="cache-card-title">Local action cache</h3>
          <p class="cache-card-note">Not recorded in the build events.</p>
        </section>
      {/if}
      {#if local?.missReasons.length}
        {@render breakdown("Local invalidation reasons", local.missReasons)}
      {/if}
      {#if stats.actionTypes.length}
        {@render breakdown("Action types", stats.actionTypes)}
      {/if}
      {#if stats.runnerTypes.length}
        {@render breakdown("Runner types", stats.runnerTypes)}
      {/if}
    </div>
  {:else}
    <p class="panel-empty">No cache statistics recorded in the build events.</p>
  {/if}
{:else if loading}
  <p class="panel-empty">Loading cache statistics…</p>
{/if}
