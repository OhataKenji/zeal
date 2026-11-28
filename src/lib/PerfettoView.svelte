<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { COMMANDS } from "./commands";
  import { formatDuration, stepTargetLabel } from "./critical_path";
  import Notice from "./Notice.svelte";
  import type { InvocationTiming, Target, TargetTab } from "./types";

  let {
    workspace,
    id,
    targets = [],
    onselecttarget,
  }: {
    workspace: string;
    id: string;
    targets?: readonly Target[];
    onselecttarget?: (label: string, tab?: TargetTab) => void;
  } = $props();

  // The parent keys this component by workspace+id; `attempt` only bumps on
  // manual retry, so prepare runs once per mount or retry.
  let attempt = $state(0);
  const viewer = $derived.by(() => {
    void attempt;
    return invoke<InvocationTiming>(COMMANDS.prepareInvocationTiming, { workspace, id });
  });
</script>

<div class="perfetto-view">
  {#await viewer}
    <p class="panel-empty" role="status">Loading timing viewer…</p>
  {:then timing}
    {#if timing.criticalPath.steps.length}
      <section class="critical-path" aria-label="Critical path">
        <header>
          <h3>Critical path</h3>
          <span>{formatDuration(timing.criticalPath.totalUs)}</span>
        </header>
        <ol>
          {#each timing.criticalPath.steps as step, index (`${index}:${step.name}`)}
            {@const targetLabel = onselecttarget ? stepTargetLabel(step, targets) : null}
            <li>
              {#if targetLabel && onselecttarget}
                <button
                  type="button"
                  title={`${step.name} — Open ${targetLabel} in Target > Actions`}
                  onclick={() => onselecttarget(targetLabel, "actions")}
                >
                  <span class="cp-name">{step.name}</span>
                  <span class="cp-target">{targetLabel}</span>
                  <span class="cp-dur">{formatDuration(step.durationUs)}</span>
                </button>
              {:else}
                <div class="cp-row" title={step.name}>
                  <span class="cp-name">{step.name}</span>
                  <span class="cp-dur">{formatDuration(step.durationUs)}</span>
                </div>
              {/if}
            </li>
          {/each}
        </ol>
      </section>
    {/if}
    <iframe
      src={timing.url}
      sandbox="allow-scripts allow-same-origin"
      title="Invocation timing"
    ></iframe>
  {:catch error}
    <Notice tone="error" onaction={() => attempt++}>{String(error)}</Notice>
  {/await}
</div>

<style>
  .perfetto-view {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 480px;
    gap: 10px;
  }
  .critical-path {
    flex: 0 0 auto;
    margin-top: 10px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface-panel);
  }
  .critical-path header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    padding: 0 6px;
    margin-bottom: 4px;
  }
  .critical-path h3 {
    margin: 0;
    font: 600 11px var(--font-sans);
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--muted-foreground);
  }
  .critical-path header span,
  .cp-dur {
    flex-shrink: 0;
    white-space: nowrap;
    font: 11px var(--mono);
    font-variant-numeric: tabular-nums;
    color: var(--muted-foreground);
  }
  .critical-path ol {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  .critical-path button,
  .cp-row {
    width: 100%;
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
    padding: 4px 6px;
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--foreground);
    font: 12px var(--font-sans);
    text-align: left;
  }
  .critical-path button {
    cursor: pointer;
  }
  .critical-path button:hover {
    background: var(--muted);
  }
  .critical-path button:focus-visible {
    outline: 2px solid var(--primary);
    outline-offset: -2px;
  }
  .cp-name {
    min-width: 0;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cp-target {
    flex-shrink: 0;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font: 11px var(--mono);
    color: var(--primary);
  }
  iframe {
    flex: 1;
    width: 100%;
    border: 0;
    min-height: 360px;
  }
</style>
