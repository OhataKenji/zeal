<script lang="ts">
  import type { Bep, Section, TargetIndex } from "../types";
  import {
    isFailedTestStatus,
    normalizeTargetLabel,
    sectionData,
    sectionMessage,
    sectionNoticeTone,
  } from "../invocation";
  import Notice from "../Notice.svelte";

  let {
    section,
    index = null,
    rerunFlags = "",
    canRerunTest,
    onselecttarget,
    onreruntest,
  }: {
    section: Section<Bep> | undefined;
    index?: TargetIndex | null;
    rerunFlags?: string;
    canRerunTest?: (pattern: string) => boolean;
    onselecttarget?: (label: string) => void;
    onreruntest?: (pattern: string, flags: string) => void;
  } = $props();
  const bep = $derived(sectionData(section));
  const labels = $derived(new Set((index?.targets ?? []).map((t) => t.label)));
</script>

{#snippet targetLabel(label: string)}
  {@const key = normalizeTargetLabel(label)}
  {#if onselecttarget && labels.has(key)}
    <button type="button" class="table-link mono" onclick={() => onselecttarget?.(key)}
      >{label}</button
    >
  {:else}
    <code>{label}</code>
  {/if}
{/snippet}

<h3 class="panel-subheading">Build results {#if bep}<span>{bep.targets.length}</span>{/if}</h3>
{#if section && section.status !== "ok"}<Notice tone={sectionNoticeTone(section)}
    >{sectionMessage(section)}</Notice
  >{/if}
{#if bep}
  {#if bep.targets.length}
    <ul class="result-list">
      {#each bep.targets as target}
        <li>
          <span class:success={target.ok} class:failed={!target.ok}>{target.ok ? "✓" : "✗"}</span>
          <div class="result-identity">
            {@render targetLabel(target.label)}
            {#if target.aspect}<div class="result-context">Aspect: <code>{target.aspect}</code></div>{/if}
            {#if target.configuration}<div class="result-context">Configuration: <code>{target.configuration}</code></div>{/if}
          </div>
          <span>{target.ok ? "Completed" : "Failed"}</span>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="panel-empty">No results recorded.</p>
  {/if}
{/if}

<h3 class="panel-subheading">Tests {#if bep}<span>{bep.tests.length}</span>{/if}</h3>
{#if bep}
  {#if bep.tests.length}
    <ul class="result-list">
      {#each bep.tests as test}
        {@const pattern = normalizeTargetLabel(test.label)}
        <li>
          <div class="result-identity">{@render targetLabel(test.label)}</div>
          <span class="result-status">{test.status}</span>
          {#if onreruntest && isFailedTestStatus(test.status)}
            <button
              type="button"
              class="button button-ghost result-action"
              disabled={canRerunTest ? !canRerunTest(pattern) : false}
              aria-label={`Re-run ${test.label}`}
              title={`Re-run: bazel test ${pattern}${rerunFlags ? ` ${rerunFlags}` : ""}`}
              onclick={() => onreruntest(pattern, rerunFlags)}
            >
              Re-run
            </button>
          {/if}
        </li>
      {/each}
    </ul>
  {:else}
    <p class="panel-empty">No results recorded.</p>
  {/if}
{/if}
