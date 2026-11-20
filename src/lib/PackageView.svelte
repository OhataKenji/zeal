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
    pkg: string | null;
    selectedLabel?: string | null;
    canRun?: boolean;
    onselect: (label: string) => void;
    onrun: (cmd: BazelCommand, pattern: string) => void;
    onopenflags: (cmd: BazelCommand, pattern: string) => void;
  } = $props();

  const targets = $derived(
    index.targets
      .filter((t) => pkg === null || t.pkg === pkg || t.pkg.startsWith(`${pkg}/`))
      .sort((a, b) => a.label.localeCompare(b.label)),
  );
  const hasSub = $derived(pkg !== null && index.targets.some((t) => t.pkg.startsWith(`${pkg}/`)));
  const pattern = $derived(pkg === null ? "//..." : hasSub ? `//${pkg}/...` : `//${pkg}:all`);
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
    {#each targets as target (target.label)}
      <tr
        class="clickable"
        class:selected={selectedLabel === target.label}
        onclick={() => onselect(target.label)}
      >
        <td class="mono">{pkg !== null && target.pkg === pkg ? `:${target.name}` : target.label}</td>
        <td class="mono dim">{target.ruleClass}</td>
      </tr>
    {:else}
      <tr><td colspan="2" class="dim">No targets in this scope.</td></tr>
    {/each}
  </tbody>
</table>
