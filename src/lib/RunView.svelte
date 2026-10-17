<script lang="ts">
  import type { SessionView } from "./session";
  import { outcome } from "./run";
  import LogPanel from "./run/LogPanel.svelte";
  import TimingPanel from "./run/TimingPanel.svelte";
  import TargetsPanel from "./run/TargetsPanel.svelte";
  import MetadataPanel from "./run/MetadataPanel.svelte";

  let { view, reload }: { view: SessionView; reload: () => void } = $props();
  const record = $derived(view.detail?.record ?? view.runs.find((run) => run.id === view.selectedRun));
  const running = $derived(view.state?.running === view.selectedRun);
  const failed = $derived(record?.finishedAt !== undefined && record.exitCode !== undefined && record.exitCode !== 0);
  const tabs = ["Logs", "Timing", "Targets", "Details"] as const;
  let active = $state<(typeof tabs)[number]>("Logs");
  let follow = $state(true);

  function tabKey(event: KeyboardEvent, index: number) {
    const next = event.key === "ArrowRight" ? (index + 1) % tabs.length
      : event.key === "ArrowLeft" ? (index + tabs.length - 1) % tabs.length
      : event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : -1;
    if (next < 0) return;
    event.preventDefault();
    active = tabs[next];
    const buttons = event.currentTarget instanceof HTMLElement
      ? event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]') : null;
    buttons?.[next].focus();
  }
</script>

<div class="run-page">
  <header class="run-summary">
    <h1>{record?.target ?? view.selectedRun}</h1>
    <div class="run-facts">
      <span class:failed>
        {record ? outcome(record, view.state?.running ?? null) : "? outcome unknown"}
      </span>
      {#if record?.finishedAt !== undefined}
        <span title="Recorded wall-clock duration">◷ {((record.finishedAt - record.startedAt) / 1000).toFixed(3)}s</span>
      {/if}
      {#if record}
        <span>{new Date(record.startedAt).toLocaleString()}</span>
        <code>bazel {record.cmd}</code>
      {/if}
    </div>
  </header>
  <div class="run-tabs" role="tablist" aria-label="Run views">
    {#each tabs as tab, index}
      <button id="run-tab-{tab}" class="btn run-tab" class:active={active === tab}
        role="tab" aria-selected={active === tab} aria-controls="run-panel-{tab}"
        tabindex={active === tab ? 0 : -1} onclick={() => active = tab}
        onkeydown={(event) => tabKey(event, index)}>{tab}</button>
    {/each}
    {#if view.detailLoading}<span class="panel-loading" role="status">Loading record…</span>{/if}
  </div>

  {#if view.detailError}
    <div class="run-notice err">詳細を読めません: {view.detailError} <button class="btn" onclick={reload}>Retry</button></div>
  {/if}

  <div class="run-panel"
    id="run-panel-{active}" role="tabpanel" aria-labelledby="run-tab-{active}">
    {#if active === "Logs"}
      <LogPanel section={view.detail?.log} liveLog={view.liveLog} liveFromStart={view.liveFromStart}
        {running} loading={view.detailLoading} bind:follow />
    {:else if active === "Timing"}
      <TimingPanel detail={view.detail} {running} loading={view.detailLoading} />
    {:else if active === "Targets"}
      <TargetsPanel section={view.detail?.bep} {running} />
    {:else}
      <MetadataPanel {record} />
    {/if}
  </div>
</div>
