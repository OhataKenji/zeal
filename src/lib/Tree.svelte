<script lang="ts">
  import { SvelteSet } from "svelte/reactivity";
  import { ancestorsOf, flatten, splitLabel, type DirNode } from "./model";

  let {
    root,
    selected,
    onselect,
  }: {
    root: DirNode;
    selected: string | null;
    onselect: (label: string) => void;
  } = $props();

  const expanded = new SvelteSet<string>();
  const rows = $derived(flatten(root, expanded));

  function toggle(dirPath: string) {
    if (expanded.has(dirPath)) expanded.delete(dirPath);
    else expanded.add(dirPath);
  }

  // A selection made anywhere reveals the target's package in the tree.
  $effect(() => {
    if (selected) {
      for (const p of ancestorsOf(splitLabel(selected).pkg)) expanded.add(p);
    }
  });
</script>

<div class="tree">
  {#each rows as r (r.key)}
    {#if r.kind === "dir"}
      <button
        class="row dir"
        style:padding-left="{r.depth * 14}px"
        onclick={() => r.dirPath && toggle(r.dirPath)}
      >
        {r.expanded ? "▾" : "▸"} {r.text}
        {#if r.sub}<span class="dim">{r.sub}</span>{/if}
      </button>
    {:else}
      <button
        class="row"
        class:sel={r.label === selected}
        style:padding-left="{r.depth * 14 + 14}px"
        onclick={() => r.label && onselect(r.label)}
      >
        {#if r.kind === "file"}<span class="dim">▸</span> {/if}{r.text}
        {#if r.sub}<span class="dim">{r.sub}</span>{/if}
      </button>
    {/if}
  {/each}
</div>
