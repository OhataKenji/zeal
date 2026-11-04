<script lang="ts">
  import type { Bep, Section } from "../types";
  import { sectionData, sectionMessage, sectionNoticeTone } from "../invocation";
  import Notice from "../Notice.svelte";

  let { section }: { section: Section<Bep> | undefined } = $props();
  const bep = $derived(sectionData(section));
</script>

<h3 class="panel-subheading">Build results {#if bep}<span>{bep.targets.length}</span>{/if}</h3>
{#if section && section.status !== "ok"}<Notice tone={sectionNoticeTone(section)}
    >{sectionMessage(section)}</Notice
  >{/if}
{#if bep}
  {#if bep.targets.length}
    <ul class="result-list">
      {#each bep.targets as target}<li><span class:success={target.ok} class:failed={!target.ok}>{target.ok ? "✓" : "✗"}</span><code>{target.label}</code><span>{target.ok ? "Completed" : "Failed"}</span></li>{/each}
    </ul>
  {:else}
    <p class="panel-empty">No results recorded.</p>
  {/if}
{/if}

<h3 class="panel-subheading">Tests {#if bep}<span>{bep.tests.length}</span>{/if}</h3>
{#if bep}
  {#if bep.tests.length}
    <ul class="result-list">
      {#each bep.tests as test}<li><code>{test.label}</code><span>{test.status}</span></li>{/each}
    </ul>
  {:else}
    <p class="panel-empty">No results recorded.</p>
  {/if}
{/if}
