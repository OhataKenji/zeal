<script lang="ts">
  import ActionPair from "./ActionPair.svelte";
  import type { packageContents } from "./model";
  import type { BazelCommand } from "./types";

  let {
    pkg,
    canRun = true,
    onselect,
    onrun,
    onopenflags,
  }: {
    pkg: ReturnType<typeof packageContents>;
    canRun?: boolean;
    onselect: (label: string) => void;
    onrun: (cmd: BazelCommand, pattern: string) => void;
    onopenflags: (cmd: BazelCommand, pattern: string) => void;
  } = $props();

  const ROW_CAP = 500;
  const { name, targets, pattern, primaryCmd } = $derived(pkg);
</script>

<div class="view-head">
  <div>
    <h1 class="view-title mono">{name}</h1>
    <div class="view-meta">
      {targets.length} discovered {targets.length === 1 ? "target" : "targets"}
    </div>
  </div>
  {#if targets.length > 0}
    <ActionPair
      {primaryCmd}
      primaryLabel={primaryCmd === "test" ? "Test" : "Build"}
      target={pattern}
      disabled={!canRun}
      {onrun}
      {onopenflags}
    />
  {/if}
</div>

<table class="lens-table">
  <thead>
    <tr><th>Target</th><th>Rule</th></tr>
  </thead>
  <tbody>
    {#each targets.slice(0, ROW_CAP) as target (target.label)}
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
            >:{target.name}</button
          >
        </td>
        <td class="mono dim">{target.ruleClass}</td>
      </tr>
    {:else}
      <tr><td colspan="2" class="dim">No targets discovered directly at this path.</td></tr>
    {/each}
  </tbody>
</table>
{#if targets.length > ROW_CAP}
  <p class="dim package-cap-note">
    Showing first {ROW_CAP} of {targets.length} targets — press ⌘K to find a target.
  </p>
{/if}
