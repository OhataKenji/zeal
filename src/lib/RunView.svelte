<script lang="ts">
  import RunHeader from "./RunHeader.svelte";
  import type { SessionView } from "./session";
  import { runStatus, sectionData, sectionMessage } from "./run";
  import LogPanel from "./run/LogPanel.svelte";
  import ProfileView from "./ProfileView.svelte";
  import TargetsPanel from "./run/TargetsPanel.svelte";

  let { view, reload }: { view: SessionView; reload: () => void } = $props();
  const selectedRun = $derived(view.selectedRun);
  const record = $derived(
    view.detail?.record ?? view.runs.find((run) => run.id === selectedRun)
      ?? (view.console?.id === selectedRun ? view.console : undefined),
  );
  const running = $derived(view.state?.running === selectedRun);
  const profile = $derived(sectionData(view.detail?.profile));
  const status = $derived(
    runStatus(record, view.state?.running ?? null, view.state?.stopping),
  );
  const sections = ["Logs", "Targets", "Details", "Timing"] as const;
  const tabs = ["All", ...sections] as const;
  let active = $state<(typeof tabs)[number]>("Logs");
  let panel = $state<HTMLDivElement>();

  $effect(() => {
    void selectedRun;
    active = view.initialRunTab;
  });

  $effect(() => {
    void active;
    void selectedRun;
    if (panel) panel.scrollTop = 0;
  });

  function tabKey(event: KeyboardEvent, index: number) {
    const next =
      event.key === "ArrowRight"
        ? (index + 1) % tabs.length
        : event.key === "ArrowLeft"
          ? (index + tabs.length - 1) % tabs.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? tabs.length - 1
              : -1;
    if (next < 0) return;
    event.preventDefault();
    active = tabs[next];
    const buttons =
      event.currentTarget instanceof HTMLElement
        ? event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>(
            '[role="tab"]',
          )
        : null;
    buttons?.[next].focus();
  }
</script>

<div class="run-page">
  <RunHeader {record} {status} fallback={selectedRun}>
    <div class="run-tabs" role="tablist" aria-label="Run views">
      {#each tabs as tab, index}
        <button
          type="button"
          class="button button-ghost tab-button"
          id="run-tab-{tab}"
          role="tab"
          aria-selected={active === tab}
          aria-controls="run-panel-{tab}"
          tabindex={active === tab ? 0 : -1}
          onclick={() => (active = tab)}
          onkeydown={(event) => tabKey(event, index)}
        >{tab}</button>
      {/each}
      {#if view.detailLoading}<span class="panel-loading" role="status"
          >Loading record…</span
        >{/if}
    </div>
  </RunHeader>

  {#if view.detailError}
    <div class="run-notice err">
      詳細を読めません: {view.detailError}
      <button type="button" class="button" onclick={reload}>Retry</button>
    </div>
  {/if}
  {#if record?.external && record.cmd === "run"}
    <p class="run-notice">Build events from a terminal. Program input, output and exit status remain in that terminal.</p>
  {/if}

  <div
    class="run-panel"
    bind:this={panel}
    class:run-all={active === "All"}
    id="run-panel-{active}"
    role="tabpanel"
    aria-labelledby="run-tab-{active}"
  >
    {#each sections as section}
      {#if active === "All" || active === section}
        <section class="run-section" aria-label={section}>
          {#if section === "Targets"}
            <TargetsPanel section={view.detail?.bep} />
          {:else if section === "Logs"}
            {#key selectedRun}
            <LogPanel
              section={view.detail?.log}
              loading={view.detailLoading}
            />
            {/key}
          {:else if section === "Details"}
            <h2 class="panel-heading">Run details</h2>
            {#if record}
              <dl class="run-details">
                <dt>Command</dt>
                <dd><code>bazel {record.cmd} {record.target}</code></dd>
                <dt>Source</dt><dd>{record.external ? "Terminal / BES" : "Zeal"}</dd>
                <dt>Workspace</dt>
                <dd><code>{record.workspace}</code></dd>
                <dt>Run ID</dt>
                <dd><code>{record.id}</code></dd>
                <dt>Started</dt>
                <dd>{new Date(record.startedAt).toLocaleString()}</dd>
                <dt>Finished</dt>
                <dd>
                  {record.finishedAt === undefined
                    ? "Not recorded"
                    : new Date(record.finishedAt).toLocaleString()}
                </dd>
                <dt>Exit code</dt>
                <dd>{record.exitCode ?? "Unknown"}</dd>
                {#if record.finishedAt !== undefined}<dt>Wall time</dt>
                  <dd>
                    {((record.finishedAt - record.startedAt) / 1000).toFixed(3)}s
                  </dd>{/if}
              </dl>
            {/if}
          {:else}
            <h2 class="panel-heading">Timing</h2>
            {#if running}<p class="panel-empty">
                Timing is available after the run finishes.
              </p>
            {:else if view.detail}
              {#if view.detail.profile.status !== "ok"}<p class="run-notice">
                  {sectionMessage(view.detail.profile)}
                </p>{/if}
              {#if profile}{#key view.detail}<ProfileView {profile} />{/key}{/if}
            {:else if view.detailLoading}<p class="panel-empty">
                Loading profile…
              </p>{/if}
          {/if}
        </section>
      {/if}
    {/each}
  </div>
</div>
