<script lang="ts">
  import type { Bep, Section } from "../types";
  import { sectionData, sectionMessage } from "../run";

  let { section, running }: { section: Section<Bep> | undefined; running: boolean } = $props();
  const bep = $derived(sectionData(section));
</script>

{#snippet results(bep: Bep)}
  {#if bep.targets.length === 0 && bep.tests.length === 0}<p class="panel-empty">No target or test results were recorded.</p>{/if}
  {#if bep.targets.length}
    <h2 class="panel-heading">Targets <span>{bep.targets.length}</span></h2>
    <ul class="result-list">
      {#each bep.targets as target}<li><span class:success={target.ok} class:failed={!target.ok}>{target.ok ? "✓" : "✗"}</span><code>{target.label}</code><span>{target.ok ? "Completed" : "Failed"}</span></li>{/each}
    </ul>
  {/if}
  {#if bep.tests.length}
    <h2 class="panel-heading">Tests <span>{bep.tests.length}</span></h2>
    <ul class="result-list">
      {#each bep.tests as test}<li><code>{test.label}</code><span>{test.status}</span></li>{/each}
    </ul>
  {/if}
{/snippet}

{#if running}<p class="panel-empty">Target and test results are available after the run finishes.</p>
{:else if section}
  {#if section.status !== "ok"}<p class="run-notice">{sectionMessage(section)}</p>{/if}
  {#if bep}{@render results(bep)}{/if}
{/if}
