<script lang="ts">
  import ActionPair from "./ActionPair.svelte";
  import { isTestRule, subtreeTargets, type DirNode } from "./model";
  import type { BazelCommand } from "./types";

  let {
    node,
    canRun = true,
    onselect,
    onrun,
    onopenflags,
  }: {
    node: DirNode;
    canRun?: boolean;
    onselect: (label: string) => void;
    onrun: (cmd: BazelCommand, pattern: string) => void;
    onopenflags: (cmd: BazelCommand, pattern: string) => void;
  } = $props();

  const ROW_CAP = 500;
  let subtreeRequested = $state(false);
  const recursive = $derived(
    node.targets.length === 0 || (node.dirs.length > 0 && subtreeRequested),
  );
  const targets = $derived(recursive ? subtreeTargets(node) : node.targets);
  const pattern = $derived(
    recursive ? (node.path ? `//${node.path}/...` : "//...") : `//${node.path}:all`,
  );
  const visible = $derived(targets.slice(0, ROW_CAP));
  const hasTests = $derived(targets.some((t) => isTestRule(t.ruleClass)));
</script>

<div class="view-head">
  <div>
    <h1 class="view-title mono">{pattern}</h1>
    <div class="view-meta">{targets.length} targets</div>
    {#if node.targets.length > 0 && node.dirs.length > 0}
      <div class="sub-switch" role="group" aria-label="Target scope">
        <button
          type="button"
          aria-pressed={!recursive}
          onclick={() => (subtreeRequested = false)}>Direct (:all) · {node.targets.length}</button
        >
        <button
          type="button"
          aria-pressed={recursive}
          onclick={() => (subtreeRequested = true)}>Subtree (/...) · {node.count}</button
        >
      </div>
    {/if}
  </div>
  <ActionPair
    primaryCmd={hasTests ? "test" : "build"}
    primaryLabel={hasTests ? "Test" : "Build"}
    target={pattern}
    disabled={!canRun}
    {onrun}
    {onopenflags}
  />
</div>

<table class="lens-table">
  <thead>
    <tr><th>Target</th><th>Rule</th></tr>
  </thead>
  <tbody>
    {#each visible as target (target.label)}
      <tr
        class="clickable"
        onclick={(e) => {
          if (!(e.target as HTMLElement).closest("button")) onselect(target.label);
        }}
      >
        <td class="mono">
          <button
            type="button"
            class="table-link mono"
            onclick={() => onselect(target.label)}
            >{#if target.pkg !== node.path}<span class="dim">//{target.pkg}</span>{/if}:{target.name}</button
          >
        </td>
        <td class="mono dim">{target.ruleClass}</td>
      </tr>
    {:else}
      <tr><td colspan="2" class="dim">No targets in this scope.</td></tr>
    {/each}
  </tbody>
</table>
{#if targets.length > ROW_CAP}
  <p class="dim package-cap-note">
    Showing first {ROW_CAP} of {targets.length} targets — select a package or press ⌘K to narrow.
  </p>
{/if}
