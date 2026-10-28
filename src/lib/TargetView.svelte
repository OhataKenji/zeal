<script lang="ts">
  import { SvelteSet } from "svelte/reactivity";
  import { MagnifyingGlass, CaretDown, CaretUp } from "phosphor-svelte";
  import type { Snapshot } from "./types";
  import { displayLabel, displayPath, relations } from "./model";
  import { openLocation } from "./state.svelte";

  let {
    snap,
    label,
    wsRoot,
    onselect,
  }: {
    snap: Snapshot;
    label: string;
    wsRoot: string;
    onselect: (label: string) => void;
  } = $props();
  const rel = $derived(relations(snap, label));
  const reverseAttrs = $derived(
    new Map(
      rel?.usedBy.map((used) => [
        used.label,
        used.attrs.filter((attr) => attr !== "deps" && attr !== "data").join(", "),
      ]) ?? [],
    ),
  );
  const sources = $derived([
    ...new Set(
      rel?.inputs
        .filter((file) => file.role === "srcs")
        .map((file) => file.path) ?? [],
    ),
  ]);
  const otherInputs = $derived([
    ...new Set(
      rel?.inputs
        .filter((file) => file.role !== "srcs")
        .map((file) => file.path) ?? [],
    ),
  ]);
  const targets = $derived(
    new Map(snap.targets.map((target) => [target.label, target])),
  );
  let query = $state("");
  const expanded = new SvelteSet<string>();
  $effect(() => {
    void label;
    void wsRoot;
    query = "";
    expanded.clear();
  });
  function matches(value: string) {
    return value.toLowerCase().includes(query.toLowerCase());
  }
  function location(file: string, line: number) {
    return `${file.startsWith(wsRoot + "/") ? file.slice(wsRoot.length + 1) : file}:${line}`;
  }
</script>

{#snippet labels(values: string[], id: string, pkg: string, attrs?: Map<string, string>)}
  {@const filtered = values.filter(matches)}
  <div class="chips">
    {#each expanded.has(id) || query ? filtered : filtered.slice(0, 12) as value (value)}
      {@const target = targets.get(value)}
      {#if target}
        <button class="chip" title={value} onclick={() => onselect(value)}>
          <span>{displayLabel(value, pkg)}</span><small
            >{target.ruleClass}</small
          >
          {#if attrs?.get(value)}<small>{attrs.get(value)}</small>{/if}
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

{#if rel}
  {@const target = rel.target}
  <header class="target-head">
    <span class="badge">{target.ruleClass}</span>
    <h1>{target.label}</h1>
    {#if target.location}
      <button
        class="link source-location"
        title={target.location.file}
        onclick={() => target.location && openLocation(target.location.file)}
      >
        {location(target.location.file, target.location.line)}
      </button>
    {/if}
  </header>
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
    <dt>srcs</dt>
    <dd>{@render files(sources, "srcs", target.pkg)}</dd>
    {#if otherInputs.length}<dt>inputs</dt>
      <dd>{@render files(otherInputs, "inputs", target.pkg)}</dd>{/if}
    <dt>data</dt>
    <dd>{@render labels(rel.data, "data", target.pkg)}</dd>
    <dt>deps</dt>
    <dd>{@render labels(rel.deps, "deps", target.pkg)}</dd>
    <dt>outs</dt>
    <dd>
      {@render files(
        [...new Set(rel.outputs.map((file) => file.path))],
        "outs",
        target.pkg,
      )}
    </dd>
    <dt>rdeps<small>depth=1</small></dt>
    <dd>
      {@render labels(
        rel.usedBy.map((used) => used.label),
        "rdeps",
        target.pkg,
        reverseAttrs,
      )}
    </dd>
  </dl>
{:else}<p class="dim">{label} is not in the snapshot</p>{/if}
