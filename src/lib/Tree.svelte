<script lang="ts">
  import { CaretDown, CaretRight } from "phosphor-svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { ancestorsOf, flatten, type DirNode } from "./model";

  let {
    root,
    activePkg,
    revealRequest = 0,
    onselect,
  }: {
    root: DirNode;
    activePkg: string;
    revealRequest?: number;
    onselect: (selection: { kind: "package"; path: string }) => void;
  } = $props();

  const expanded = new SvelteSet<string>();
  const rows = $derived(flatten(root, expanded));

  function toggle(dirPath: string) {
    if (expanded.has(dirPath)) {
      expanded.delete(dirPath);
    } else {
      expanded.add(dirPath);
    }
  }

  // A selection made anywhere reveals the package's ancestors in the tree,
  // but never opens the package's own children.
  $effect(() => {
    // Re-picking the same package must reveal it after a manual collapse.
    void revealRequest;
    for (const p of ancestorsOf(activePkg).slice(0, -1)) {
      expanded.add(p);
    }
  });
</script>

<div class="tree">
  {#each rows as r (r.path)}
    <button
      class="row dir"
      class:selected={r.path === activePkg}
      aria-current={r.path === activePkg ? "true" : undefined}
      aria-expanded={r.hasChildren ? r.expanded : undefined}
      title={r.path}
      style:padding-left="{8 + r.depth * 12}px"
      onclick={() => {
        onselect({ kind: "package", path: r.path });
        if (r.hasChildren) toggle(r.path);
      }}
    >
      {#if r.hasChildren}
        {#if r.expanded}<CaretDown size={10} />{:else}<CaretRight size={10} />{/if}
      {:else}
        <span class="tree-caret-slot" aria-hidden="true"></span>
      {/if}
      <span>{r.name}</span>
      <small>{r.count}</small>
    </button>
  {/each}
</div>
