<script lang="ts">
  import { SvelteSet } from "svelte/reactivity";
  import { MagnifyingGlass, CaretDown, CaretUp } from "phosphor-svelte";
  import type { TargetDetail, TargetIndex } from "./types";
  import { displayLabel, displayPath, relations } from "./model";
  import { openLocation } from "./state.svelte";
  import Notice from "./Notice.svelte";

  let {
    index,
    detail,
    wsRoot,
    onselect,
  }: {
    index: TargetIndex;
    detail: TargetDetail;
    wsRoot: string;
    onselect: (label: string) => void;
  } = $props();
  const rel = $derived(relations(detail));
  const target = $derived(rel.target);
  const targets = $derived(
    new Map(index.targets.map((target) => [target.label, target])),
  );
  let query = $state("");
  let locationError = $state<string | null>(null);
  const expanded = new SvelteSet<string>();
  $effect(() => {
    void target;
    locationError = null;
  });
  async function openSource() {
    const selected = target;
    if (!selected.location) return;
    locationError = null;
    try {
      await openLocation(selected.location.file);
    } catch (error) {
      if (target === selected) locationError = String(error);
    }
  }
  function matches(value: string) {
    return value.toLowerCase().includes(query.toLowerCase());
  }
  function location(file: string, line: number) {
    return `${file.startsWith(wsRoot + "/") ? file.slice(wsRoot.length + 1) : file}:${line}`;
  }
</script>

{#snippet labels(values: string[], id: string, pkg: string)}
  {@const filtered = values.filter(matches)}
  <div class="chips">
    {#each expanded.has(id) || query ? filtered : filtered.slice(0, 12) as value (value)}
      {@const target = targets.get(value)}
      {#if target}
        <button class="chip" title={value} onclick={() => onselect(value)}>
          <span>{displayLabel(value, pkg)}</span><small
            >{target.ruleClass}</small
          >
        </button>
      {:else}<span class="chip" title={value}>{displayLabel(value, pkg)}</span
        >{/if}
    {/each}
    {#if !query && filtered.length > 12}
      <button
        class="chip more"
        aria-expanded={expanded.has(id)}
        onclick={() =>
          expanded.has(id) ? expanded.delete(id) : expanded.add(id)}
      >
        {#if expanded.has(id)}Show less <CaretUp
            size={10}
          />{:else}+{filtered.length - 12} more <CaretDown size={10} />{/if}
      </button>
    {/if}
    {#if filtered.length === 0}<span class="dim"
        >{query ? "No matching labels" : "None"}</span
      >{/if}
  </div>
{/snippet}

{#snippet files(values: string[], group: string, pkg: string)}
  {@const filtered = values.filter(matches)}
  <div class="chips">
    {#each expanded.has(group) || query ? filtered : filtered.slice(0, 16) as path (path)}
      <span class="chip file" class:local={group === "outs"} title={path}
        >{displayPath(path, pkg)}</span
      >
    {/each}
    {#if !query && filtered.length > 16}
      <button
        class="chip more"
        aria-expanded={expanded.has(group)}
        onclick={() =>
          expanded.has(group) ? expanded.delete(group) : expanded.add(group)}
      >
        {expanded.has(group) ? "Show less" : `+${filtered.length - 16} more`}
        <CaretDown size={10} />
      </button>
    {/if}
    {#if filtered.length === 0}<span class="dim"
        >{query ? "No matching files" : "None"}</span
      >{/if}
  </div>
{/snippet}

  <header class="target-head">
    <span class="badge">{target.ruleClass}</span>
    <h1>{target.label}</h1>
    {#if target.location}
      <button
        class="link source-location"
        title={target.location.file}
        onclick={openSource}
      >
        {location(target.location.file, target.location.line)}
      </button>
    {/if}
  </header>
  {#if locationError}<Notice tone="error">Could not open source file: {locationError}</Notice>{/if}
  <div class="attributes-head">
    <h2>Attributes</h2>
    <label class="relation-filter"
      ><MagnifyingGlass size={13} /><input
        class="input"
        aria-label="Filter labels and files"
        placeholder="Filter labels…"
        bind:value={query}
      /></label
    >
  </div>
  <dl class="relations">
    {#each rel.attributes as attribute (attribute.name)}
      {@const matchingLabels = attribute.labels.filter(matches)}
      {@const matchingFiles = attribute.files.filter(matches)}
      <dt>{attribute.name}</dt>
      <dd>
        {#if matchingLabels.length}
          {@render labels(attribute.labels, `${attribute.name}-labels`, target.pkg)}
        {/if}
        {#if matchingFiles.length}
          {@render files(attribute.files, `${attribute.name}-files`, target.pkg)}
        {/if}
        {#if !matchingLabels.length && !matchingFiles.length}
          <span class="dim">{query ? "No matching labels or files" : "None"}</span>
        {/if}
      </dd>
    {/each}
    <dt>outs</dt>
    <dd>
      {@render files(
        [...new Set(rel.outputs.map((file) => file.path))],
        "outs",
        target.pkg,
      )}
    </dd>
  </dl>
