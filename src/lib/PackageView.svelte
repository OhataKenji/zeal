<script lang="ts">
  import ActionPair from "./ActionPair.svelte";
  import type { BazelCommand, TargetIndex } from "./types";

  let {
    index,
    pkg,
    selectedLabel = null,
    canRun = true,
    onselect,
    onrun,
    onopenflags,
  }: {
    index: TargetIndex;
    pkg: string;
    selectedLabel?: string | null;
    canRun?: boolean;
    onselect: (label: string) => void;
    onrun: (cmd: BazelCommand, pattern: string) => void;
    onopenflags: (cmd: BazelCommand, pattern: string) => void;
  } = $props();

  const ROW_CAP = 500;
  const sortedTargets = $derived(
    [...index.targets].sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0)),
  );
  const targets = $derived(
    pkg === ""
      ? sortedTargets
      : sortedTargets.filter((t) => t.pkg === pkg || t.pkg.startsWith(`${pkg}/`)),
  );
  const visible = $derived(targets.slice(0, ROW_CAP));
  const hasSub = $derived(index.targets.some((t) => t.pkg.startsWith(`${pkg}/`)));
  const pattern = $derived(pkg === "" ? "//..." : hasSub ? `//${pkg}/...` : `//${pkg}:all`);
  const hasTests = $derived(targets.some((t) => t.ruleClass.endsWith("_test")));
</script>

<div class="view-head">
  <div>
    <h1 class="view-title mono">{pattern}</h1>
    <div class="view-meta">{targets.length} targets</div>
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
        class:selected={selectedLabel === target.label}
        onclick={(e) => {
          if (!(e.target as HTMLElement).closest("button")) onselect(target.label);
        }}
      >
        <td class="mono">
          <button
            type="button"
            class="table-link mono"
            onclick={() => onselect(target.label)}
          >{pkg !== "" && target.pkg === pkg ? `:${target.name}` : target.label}</button>
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
