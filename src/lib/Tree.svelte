<script lang="ts">
  import { CaretDown, CaretRight } from "phosphor-svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { ancestorsOf, splitLabel, type DirNode } from "./model";
  import type { ExplorerSelection } from "./types";

  let {
    root,
    selection,
    onselect,
  }: {
    root: DirNode;
    selection: ExplorerSelection;
    onselect: (selection: ExplorerSelection) => void;
  } = $props();

  const expanded = new SvelteSet<string>();

  function toggle(dirPath: string) {
    if (expanded.has(dirPath)) {
      expanded.delete(dirPath);
    } else {
      expanded.add(dirPath);
    }
  }

  function selectDir(dir: DirNode) {
    // Clicking the already-selected package toggles it instead of re-selecting.
    if (selection?.kind === "package" && selection.path === dir.path) {
      toggle(dir.path);
      return;
    }
    expanded.add(dir.path);
    onselect({ kind: "package", path: dir.path });
  }

  // A selection made anywhere — palette, history, or returning to the page —
  // restores the expanded ancestors: a target expands through its own
  // package so its leaves show, while a package expands only its ancestors
  // so a folder the user collapsed stays collapsed.
  $effect(() => {
    const paths =
      selection?.kind === "target"
        ? ancestorsOf(splitLabel(selection.label).pkg)
        : selection?.kind === "package"
          ? ancestorsOf(selection.path).slice(0, -1)
          : [];
    for (const path of paths) {
      expanded.add(path);
    }
  });
</script>

{#snippet children(node: DirNode, depth: number)}
  {#each node.dirs as dir (dir.path)}
    {@const open = expanded.has(dir.path)}
    {@const isPkg = selection?.kind === "package" && selection.path === dir.path}
    <div
      class="tree-row dir"
      class:selected={isPkg}
      class:grouping={dir.targets.length === 0}
      style:padding-left="{8 + depth * 12}px"
    >
      <button
        type="button"
        class="tree-caret"
        aria-label={open ? `Collapse //${dir.path}` : `Expand //${dir.path}`}
        aria-expanded={open}
        onclick={() => toggle(dir.path)}
      >
        {#if open}<CaretDown size={10} />{:else}<CaretRight size={10} />{/if}
      </button>
      <button
        type="button"
        class="tree-body"
        aria-current={isPkg ? "true" : undefined}
        title={`//${dir.path}`}
        onclick={() => selectDir(dir)}
      >
        <span class="tree-name">{dir.name}/</span>
        <small>{dir.count}</small>
      </button>
    </div>
    {#if open}{@render children(dir, depth + 1)}{/if}
  {/each}
  {#each node.targets as target (target.label)}
    {@const isTarget = selection?.kind === "target" && selection.label === target.label}
    <button
      type="button"
      class="tree-row target"
      class:selected={isTarget}
      aria-current={isTarget ? "true" : undefined}
      title={`${target.label} (${target.ruleClass})`}
      style:padding-left="{8 + depth * 12}px"
      onclick={() => onselect({ kind: "target", label: target.label })}
    >
      <span class="tree-name"><span class="dim">:</span>{target.name}</span>
    </button>
  {/each}
{/snippet}

<div class="tree">
  {@render children(root, 0)}
</div>
