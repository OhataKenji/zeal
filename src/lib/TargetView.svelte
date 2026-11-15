<script lang="ts">
  import { SvelteSet } from "svelte/reactivity";
  import { CaretDown, CaretUp } from "phosphor-svelte";
  import type {
    TargetActions,
    TargetDetail,
    TargetIndex,
    TargetOutputs,
  } from "./types";
  import { displayLabel, displayPath, relations } from "./model";
  import ActionsSection from "./TargetActions.svelte";

  let {
    index,
    detail,
    outputs,
    outputsError,
    actions,
    actionsError,
    wsRoot,
    onselect,
  }: {
    index: TargetIndex;
    detail: TargetDetail;
    outputs: TargetOutputs | null;
    outputsError: string | null;
    actions: TargetActions | null;
    actionsError: string | null;
    wsRoot: string;
    onselect: (label: string) => void;
  } = $props();
  const rel = $derived(relations(detail));
  const target = $derived(rel.target);
  const targets = $derived(
    new Map(index.targets.map((target) => [target.label, target])),
  );
  const expanded = new SvelteSet<string>();
  function location(file: string, line: number) {
    return `${file.startsWith(wsRoot + "/") ? file.slice(wsRoot.length + 1) : file}:${line}`;
  }
</script>

{#snippet labels(values: string[], id: string, pkg: string)}
  <div class="chips">
    {#each expanded.has(id) ? values : values.slice(0, 12) as value (value)}
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
    {#if values.length > 12}
      <button
        class="chip more"
        aria-expanded={expanded.has(id)}
        onclick={() =>
          expanded.has(id) ? expanded.delete(id) : expanded.add(id)}
      >
        {#if expanded.has(id)}Show less <CaretUp
            size={10}
          />{:else}+{values.length - 12} more <CaretDown size={10} />{/if}
      </button>
    {/if}
  </div>
{/snippet}

{#snippet files(values: string[], group: string, pkg: string)}
  <div class="chips">
    {#each expanded.has(group) ? values : values.slice(0, 16) as path (path)}
      <span class="chip file" title={path}
        >{displayPath(path, pkg)}</span
      >
    {/each}
    {#if values.length > 16}
      <button
        class="chip more"
        aria-expanded={expanded.has(group)}
        onclick={() =>
          expanded.has(group) ? expanded.delete(group) : expanded.add(group)}
      >
        {expanded.has(group) ? "Show less" : `+${values.length - 16} more`}
        <CaretDown size={10} />
      </button>
    {/if}
    {#if values.length === 0}<span class="dim">None</span>{/if}
  </div>
{/snippet}

  <header class="target-head">
    <h1>{target.label}</h1>
    <span class="badge">{target.ruleClass}</span>
    {#if target.location}
      <span class="source-location" title={target.location.file}>
        {location(target.location.file, target.location.line)}
      </span>
    {/if}
  </header>
  <section class="outputs">
    <h2>Outputs</h2>
    {#if outputsError}
      <p class="dim">Could not load outputs: {outputsError}</p>
    {:else if outputs === null}
      <p class="dim">Loading outputs…</p>
    {:else if outputs.paths.length === 0}
      <p class="dim">None</p>
    {:else}
      <div class="chips">
        {#each outputs.paths as path (path)}
          <span class="chip file" title={path}>{path}</span>
        {/each}
        {#if outputs.truncated}<span class="chip more" title="More outputs not shown"
            >…</span
          >{/if}
      </div>
    {/if}
  </section>
  <h2 class="attributes-head">Attributes</h2>
  <dl class="relations">
    {#each rel.attributes as attribute (attribute.name)}
      <dt>{attribute.name}</dt>
      <dd>
        {#if attribute.labels.length}
          {@render labels(attribute.labels, `${attribute.name}-labels`, target.pkg)}
        {/if}
        {#if attribute.files.length}
          {@render files(attribute.files, `${attribute.name}-files`, target.pkg)}
        {/if}
        {#if !attribute.labels.length && !attribute.files.length}
          <span class="dim">None</span>
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
  {#key target.label}
    <ActionsSection actions={actions} error={actionsError} />
  {/key}
