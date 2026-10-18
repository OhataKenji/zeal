<script lang="ts">
  import type { SessionView } from "./session";
  import { outcome, sectionData, sectionMessage } from "./run";
  import LogPanel from "./run/LogPanel.svelte";
  import ProfileView from "./ProfileView.svelte";

  let { view, reload }: { view: SessionView; reload: () => void } = $props();
  const record = $derived(view.detail?.record ?? view.runs.find((run) => run.id === view.selectedRun));
  const profile = $derived(sectionData(view.detail?.profile));
  const failed = $derived(record?.outcome === "failed");
  const tabs = ["Logs", "Timing", "Details"] as const;
  let active = $state<(typeof tabs)[number]>("Logs");

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
        {record ? outcome(record) : "? outcome unknown"}
      </span>
      {#if record?.elapsedMs != null}
        <span title="実行時間（コマンドの起動から終了まで）">◷ {(record.elapsedMs / 1000).toFixed(3)}s</span>
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
      <LogPanel section={view.detail?.log} loading={view.detailLoading} />
    {:else if active === "Timing"}
      {#if view.detail}
        {#if view.detail.profile.status !== "ok"}<p class="run-notice">{sectionMessage(view.detail.profile)}</p>{/if}
        {#if profile}{#key view.detail}<ProfileView {profile} />{/key}{/if}
      {:else if view.detailLoading}<p class="panel-empty">Loading profile…</p>{/if}
    {:else}
      <h2 class="panel-heading">Run details</h2>
      {#if record}
        <dl class="run-details">
          <dt>Command</dt><dd><code>bazel {record.cmd} {record.target}</code></dd>
          <dt>Workspace</dt><dd><code>{record.workspace}</code></dd>
          <dt>Run ID</dt><dd><code>{record.id}</code></dd>
          <dt>Started</dt><dd>{new Date(record.startedAt).toLocaleString()}</dd>
          <dt>Finished</dt><dd>{record.finishedAt === undefined ? "Not recorded" : new Date(record.finishedAt).toLocaleString()}</dd>
          <dt>Exit code</dt><dd>{record.exitCode ?? "Unknown"}</dd>
          {#if record.elapsedMs != null}<dt>実行時間</dt><dd>{(record.elapsedMs / 1000).toFixed(3)}s</dd>{/if}
        </dl>
      {/if}
    {/if}
  </div>
</div>
