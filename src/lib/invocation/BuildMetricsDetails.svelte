<script lang="ts">
  import type { Bep, Section } from "../types";
  import { sectionData, sectionMessage, sectionNoticeTone } from "../invocation";
  import Notice from "../Notice.svelte";

  let { section, loading }: {
    section: Section<Bep> | undefined;
    loading: boolean;
  } = $props();

  const bep = $derived(sectionData(section));
  const local = $derived(bep?.localActionCache);
  const artifacts = $derived(bep?.outputArtifactsFromActionCache);
  const runners = $derived(bep?.runnerCounts ?? []);
  const runnerRows = $derived(runners.filter((runner) => runner.name !== "total"));
  const runnerTotals = $derived(runners.filter((runner) => runner.name === "total"));
  const remote = $derived(bep?.remoteAnalysisCache);
</script>

<h3 class="panel-subheading">Build metrics</h3>
{#if section && section.status !== "ok"}
  <Notice tone={sectionNoticeTone(section)}>{sectionMessage(section)}</Notice>
{/if}
{#if bep}
  <section class="invocation-command-block">
    <h4>Local action cache</h4>
    {#if local}
      <dl class="invocation-details build-metrics-list">
        <dt>Hits</dt>
        <dd>{local.hits}</dd>
        <dt>Misses</dt>
        <dd>{local.misses}</dd>
        {#each local.missReasons as reason}
          <dt>{reason.name}</dt>
          <dd>{reason.count}</dd>
        {/each}
      </dl>
    {:else}
      <p class="panel-empty">Not recorded</p>
    {/if}
  </section>

  <section class="invocation-command-block">
    <h4>Output artifacts from action cache</h4>
    {#if artifacts}
      <dl class="invocation-details build-metrics-list">
        <dt>Count (files)</dt>
        <dd>{artifacts.count}</dd>
        <dt>Size (bytes)</dt>
        <dd>{artifacts.sizeInBytes}</dd>
      </dl>
    {:else}
      <p class="panel-empty">Not recorded</p>
    {/if}
  </section>

  <section class="invocation-command-block">
    <h4>Runner count</h4>
    {#if runners.length}
      <dl class="invocation-details build-metrics-list">
        {#each runnerRows as runner}
          <dt>{runner.name}</dt>
          <dd>{runner.count}</dd>
        {/each}
        {#each runnerTotals as runner}
          <dt>Total</dt>
          <dd>{runner.count}</dd>
        {/each}
      </dl>
    {:else}
      <p class="panel-empty">Not recorded</p>
    {/if}
  </section>

  <section class="invocation-command-block">
    <h4>Test cache results</h4>
    {#if bep.tests.length}
      {#each bep.tests as test}
        <h5 class="build-metrics-target">{test.label}</h5>
        <dl class="invocation-details build-metrics-list">
          <dt>Cached test actions</dt>
          <dd>{test.totalNumCached ?? "Not recorded"}</dd>
          <dt>Shard attempts</dt>
          <dd>{test.totalRunCount ?? "Not recorded"}</dd>
        </dl>
      {/each}
    {:else}
      <p class="panel-empty">Not recorded</p>
    {/if}
  </section>

  <section class="invocation-command-block">
    <h4>Remote analysis cache</h4>
    {#if remote}
      <dl class="invocation-details build-metrics-list">
        <dt>Cache hits</dt>
        <dd>{remote.cacheHits}</dd>
        <dt>Cache misses</dt>
        <dd>{remote.cacheMisses}</dd>
      </dl>
    {:else}
      <p class="panel-empty">Not recorded</p>
    {/if}
  </section>
{:else if loading}
  <p class="panel-empty">Loading build metrics…</p>
{/if}
