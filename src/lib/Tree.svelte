<script lang="ts">
  import { CaretDown, CaretRight, Circle } from "phosphor-svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { ancestorsOf, flatten, splitLabel, type DirNode } from "./model";

  let {
    root,
    selected,
    revealRequest = 0,
    onselect,
  }: {
    root: DirNode;
    selected: string | null;
    revealRequest?: number;
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
    // Re-picking the same target must reveal it after a manual collapse.
    void revealRequest;
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
        style:padding-left="{r.depth * 12}px"
        aria-expanded={r.expanded}
        onclick={() => r.dirPath && toggle(r.dirPath)}
      >
        {#if r.expanded}<CaretDown size={10} />{:else}<CaretRight
            size={10}
          />{/if}<span>{r.text}</span>
      </button>
    {:else}
      <button
        title={r.label ?? r.text}
        class="row"
        class:selected={r.label === selected}
        class:test={r.ruleClass?.endsWith("_test")}
        aria-current={r.label === selected ? "true" : undefined}
        style:padding-left="{r.depth * 12 + 2}px"
        onclick={() => r.label && onselect(r.label)}
      >
        <Circle size={7} weight="fill" /><span>{r.text}</span>
        <small>{r.ruleClass}</small>
      </button>
    {/if}
  {/each}
</div>
