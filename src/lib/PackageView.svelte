<script lang="ts">
  import ActionPair from "./ActionPair.svelte";
  import { isTestRule } from "./model";
  import type { BazelCommand, Target, TargetIndex } from "./types";

  let {
    index,
    pkg,
    canRun = true,
    onselect,
    onrun,
    onopenflags,
  }: {
    index: TargetIndex;
    pkg: string;
    canRun?: boolean;
    onselect: (label: string) => void;
    onrun: (cmd: BazelCommand, pattern: string) => void;
    onopenflags: (cmd: BazelCommand, pattern: string) => void;
  } = $props();

  const ROW_CAP = 500;
  function byLabel(a: Target, b: Target): number {
    return a.label < b.label ? -1 : a.label > b.label ? 1 : 0;
  }
  // Filter before sorting so a package click does not re-sort the workspace.
  const directTargets = $derived(index.targets.filter((t) => t.pkg === pkg).sort(byLabel));
  const subTargets = $derived(
    index.targets
      .filter((t) => t.pkg !== pkg && (pkg === "" || t.pkg.startsWith(`${pkg}/`)))
      .sort(byLabel),
  );
  let subtreeRequested = $state(false);
  const recursive = $derived(
    !directTargets.length || (subTargets.length > 0 && subtreeRequested),
  );
  const targets = $derived(recursive ? [...directTargets, ...subTargets] : directTargets);
  const pattern = $derived(recursive ? (pkg ? `//${pkg}/...` : "//...") : `//${pkg}:all`);
  const subtreeTotal = $derived(directTargets.length + subTargets.length);
  const visible = $derived(targets.slice(0, ROW_CAP));
  const hasTests = $derived(targets.some((t) => isTestRule(t.ruleClass)));
</script>

<div class="view-head">
  <div>
    <h1 class="view-title mono">{pattern}</h1>
    <div class="view-meta">{targets.length} targets</div>
    {#if directTargets.length > 0 && subTargets.length > 0}
      <div class="sub-switch" role="group" aria-label="Target scope">
        <button
          type="button"
          aria-pressed={!recursive}
          class:active={!recursive}
          onclick={() => (subtreeRequested = false)}>Direct (:all) · {directTargets.length}</button
        >
        <button
          type="button"
          aria-pressed={recursive}
          class:active={recursive}
          onclick={() => (subtreeRequested = true)}>Subtree (/...) · {subtreeTotal}</button
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
            >{#if target.pkg !== pkg}<span class="dim">//{target.pkg}</span>{/if}:{target.name}</button
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
