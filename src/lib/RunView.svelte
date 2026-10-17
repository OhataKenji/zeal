<script lang="ts">
  import { tick } from "svelte";
  import type { SessionView } from "./session";
  import { outcome, sectionData, sectionMessage } from "./run";
  import ProfileView from "./ProfileView.svelte";

  let { view, reload }: { view: SessionView; reload: () => void } = $props();
  const record = $derived(view.detail?.record ?? view.runs.find((run) => run.id === view.selectedRun));
  const running = $derived(view.state?.running === view.selectedRun);
  const profile = $derived(sectionData(view.detail?.profile));
  const bep = $derived(sectionData(view.detail?.bep));
  const log = $derived(running ? view.liveLog : view.detail ? sectionData(view.detail.log) : view.liveLog || null);
  const failed = $derived(record?.finishedAt !== undefined && record.exitCode !== undefined && record.exitCode !== 0);
  const tabs = ["Logs", "Timing", "Targets", "Details"] as const;
  let active = $state<(typeof tabs)[number]>("Logs");
  let logElement = $state<HTMLTextAreaElement>();
  let follow = $state(true);

  $effect(() => {
    void log;
    if (active === "Logs" && follow) void tick().then(() => {
      if (logElement) logElement.scrollTop = logElement.scrollHeight;
    });
  });

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
      {#if running && !view.liveFromStart}<p class="run-notice">接続後の出力を表示中。完了後に保存済みの全 log を読み込みます。</p>{/if}
      {#if !running && view.detail && view.detail.log.status !== "ok"}
        <p class="run-notice">{sectionMessage(view.detail.log)}</p>
      {/if}
      {#if log !== null}
        <div class="log-panel">
          <div class="log-toolbar">
            <strong>Build log</strong>
            {#if running}<span class="log-live">● Live</span>{/if}
            <label><input type="checkbox" bind:checked={follow} /> Follow output</label>
          </div>
          <textarea class="run-log" aria-label="Build stdout and stderr" readonly wrap="off"
            bind:this={logElement} value={log || (running ? "出力待ち" : "(empty log)")}
            onscroll={() => {
              if (logElement) follow = logElement.scrollHeight - logElement.scrollTop - logElement.clientHeight < 24;
            }}></textarea>
        </div>
      {:else if view.detailLoading}<p class="panel-empty">Loading build log…</p>{/if}
    {:else if active === "Timing"}
      {#if running}<p class="panel-empty">Timing is available after the run finishes.</p>
      {:else if view.detail}
        {#if view.detail.profile.status !== "ok"}<p class="run-notice">{sectionMessage(view.detail.profile)}</p>{/if}
        {#if profile}{#key view.detail}<ProfileView {profile} />{/key}{/if}
      {:else if view.detailLoading}<p class="panel-empty">Loading profile…</p>{/if}
    {:else if active === "Targets"}
      {#if running}<p class="panel-empty">Target and test results are available after the run finishes.</p>
      {:else if view.detail}
        {#if view.detail.bep.status !== "ok"}<p class="run-notice">{sectionMessage(view.detail.bep)}</p>{/if}
        {#if bep}
          {#if bep.targets.length === 0 && bep.tests.length === 0}<p class="panel-empty">No target or test results were recorded.</p>{/if}
          {#if bep.targets.length}
            <h2 class="panel-heading">Targets <span>{bep.targets.length}</span></h2>
            <ul class="result-list">
              {#each bep.targets as target}<li><span class:success={target.ok} class:failed={!target.ok}>{target.ok ? "✓" : "✗"}</span><code>{target.label}</code><span>{target.ok ? "Completed" : "Failed"}</span></li>{/each}
            </ul>
          {/if}
          {#if bep.tests.length}
            <h2 class="panel-heading">Tests <span>{bep.tests.length}</span></h2>
            <ul class="result-list">
              {#each bep.tests as test}<li><code>{test.label}</code><span>{test.status}</span></li>{/each}
            </ul>
          {/if}
        {/if}
      {/if}
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
          {#if record.finishedAt !== undefined}<dt>Wall time</dt><dd>{((record.finishedAt - record.startedAt) / 1000).toFixed(3)}s</dd>{/if}
        </dl>
      {/if}
    {/if}
  </div>
</div>
