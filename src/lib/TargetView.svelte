<script lang="ts">
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
  const known = $derived(new Set(snap.targets.map((t) => t.label)));

  function locText(file: string, line: number): string {
    const f =
      wsRoot && file.startsWith(wsRoot + "/") ? file.slice(wsRoot.length + 1) : file;
    return `${f}:${line}`;
  }
</script>

{#if rel}
  {@const t = rel.target}
  <div class="target-head">
    <strong>{t.label}</strong>
    <span class="badge">{t.ruleClass}</span>
    {#if t.location}
      <button
        class="link dim"
        title={t.location.file}
        onclick={() => t.location && openLocation(t.location.file)}
      >
        {locText(t.location.file, t.location.line)}
      </button>
    {/if}
  </div>
  <dl class="rel">
    <dt>deps →</dt>
    <dd>
      {#each rel.deps as l, i (l)}
        {#if i > 0} · {/if}
        {#if known.has(l)}
          <button class="link" onclick={() => onselect(l)}>{displayLabel(l, t.pkg)}</button>
        {:else}
          <span>{displayLabel(l, t.pkg)}</span>
        {/if}
      {/each}
      {#if rel.deps.length === 0}<span class="dim">—</span>{/if}
    </dd>
    <dt>data →</dt>
    <dd>
      {#each rel.data as l, i (l)}
        {#if i > 0} · {/if}
        {#if known.has(l)}
          <button class="link" onclick={() => onselect(l)}>{displayLabel(l, t.pkg)}</button>
        {:else}
          <span>{displayLabel(l, t.pkg)}</span>
        {/if}
      {/each}
      {#if rel.data.length === 0}<span class="dim">—</span>{/if}
    </dd>
    <dt>used by ←</dt>
    <dd>
      {#each rel.usedBy as u, i (u.label + u.attr)}
        {#if i > 0} · {/if}
        <button class="link" onclick={() => onselect(u.label)}
          >{displayLabel(u.label, t.pkg)}</button
        >
        <span class="dim"
          >{u.ruleClass}{u.attr !== "deps" && u.attr !== "data" ? ` ${u.attr}` : ""}</span
        >
      {/each}
      {#if rel.usedBy.length === 0}<span class="dim">—</span>{/if}
    </dd>
    <dt>files</dt>
    <dd>
      {[...new Set(rel.inputs.map((f) => displayPath(f.path, t.pkg)))].join(", ")}
      {#if rel.outputs.length > 0}
        {" → "}
        {[...new Set(rel.outputs.map((f) => displayPath(f.path, t.pkg)))].join(", ")}
      {/if}
      {#if rel.inputs.length === 0 && rel.outputs.length === 0}
        <span class="dim">—</span>
      {/if}
    </dd>
  </dl>
{:else}
  <p class="dim">{label} は snapshot にありません</p>
{/if}
