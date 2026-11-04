<script lang="ts">
  import { CaretDown, CaretRight } from "phosphor-svelte";
  import { SvelteSet } from "svelte/reactivity";
  import type { ExplorerSelection } from "./types";
  import { ancestorsOf, flatten, splitLabel, type DirNode } from "./model";

  let {
    root,
    selection,
    revealRequest = 0,
    onselect,
  }: {
    root: DirNode;
    selection: ExplorerSelection;
    revealRequest?: number;
    onselect: (selection: ExplorerSelection) => void;
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

  // A selection made anywhere reveals the target's package in the tree.
  $effect(() => {
    // Re-picking the same target must reveal it after a manual collapse.
    void revealRequest;
    if (selection?.kind === "target") {
      for (const p of ancestorsOf(splitLabel(selection.label).pkg)) {
        expanded.add(p);
      }
    }
  });
</script>

<div class="tree">
  {#each rows as r (r.key)}
    {#if r.kind === "dir"}
      <button
        class="row dir"
        style:padding-left="{r.depth * 12}px"
        class:selected={selection?.kind === "package" && r.dirPath === selection.path}
        aria-current={selection?.kind === "package" && r.dirPath === selection.path ? "true" : undefined}
        aria-expanded={r.expanded}
        onclick={() => {
          if (r.dirPath !== undefined) {
            toggle(r.dirPath);
            if (r.hasTargets) onselect({ kind: "package", path: r.dirPath });
          }
        }}
      >
        {#if r.expanded}<CaretDown size={10} />{:else}<CaretRight
            size={10}
          />{/if}<span>{r.text}</span>
      </button>
    {:else}
      <button
        title={r.label ?? r.text}
        class="row"
        class:selected={selection?.kind === "target" && r.label === selection.label}
        aria-current={selection?.kind === "target" && r.label === selection.label ? "true" : undefined}
        style:padding-left="{r.depth * 12 + 2}px"
        onclick={() => r.label && onselect({ kind: "target", label: r.label })}
      >
        <span>{r.text}</span>
        <small>{r.ruleClass}</small>
      </button>
    {/if}
  {/each}
</div>
