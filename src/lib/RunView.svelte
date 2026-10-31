<script lang="ts">
  import RunHeader from "./RunHeader.svelte";
  import type { SessionView } from "./session";
  import { runStatus, sectionData, sectionMessage } from "./run";
  import LogPanel from "./run/LogPanel.svelte";
  import ProfileView from "./ProfileView.svelte";
  import TargetsPanel from "./run/TargetsPanel.svelte";
  import { formatOptions } from "./run/options";

  let { view, reload, reloadProfile }: {
    view: SessionView;
    reload: () => void;
    reloadProfile: () => void;
  } = $props();
  const selectedRun = $derived(view.selectedRun);
  const record = $derived(
    view.detail?.record ?? view.runs.find((run) => run.id === selectedRun),
  );
  const running = $derived(view.state?.running === selectedRun);
  const profile = $derived(sectionData(view.profile ?? undefined));
  const bep = $derived(sectionData(view.detail?.bep));
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
    <div class="run-notice error">
      Could not load run details: {view.detailError}
      <button type="button" class="button" onclick={reload}>Retry</button>
    </div>
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
                <dd>{#if record.cmd}<code>bazel {record.cmd} {record.target ?? "(target not recorded)"}</code>{:else}Not recorded{/if}</dd>
                <dt>Workspace</dt>
                <dd><code>{record.workspace ?? "Not recorded"}</code></dd>
                <dt>Run ID</dt>
                <dd><code>{record.id}</code></dd>
                <dt>Started</dt>
                <dd>{record.startedAt === undefined ? "Not recorded" : new Date(record.startedAt).toLocaleString()}</dd>
                <dt>Finished</dt>
                <dd>
                  {record.finishedAt === undefined
                    ? "Not recorded"
                    : new Date(record.finishedAt).toLocaleString()}
                </dd>
                <dt>Exit code</dt>
                <dd>{record.exitCode ?? "Unknown"}</dd>
                {#if record.startedAt !== undefined && record.finishedAt !== undefined}<dt>Wall time</dt>
                  <dd>
                    {((record.finishedAt - record.startedAt) / 1000).toFixed(3)}s
                  </dd>{/if}
              </dl>
              <h3 class="run-arguments-heading">Command lines</h3>
              <section class="run-command-block">
                <h4>Original command line</h4>
                {#if bep?.originalCommandLine !== undefined}
                  <code class="run-command-code">{bep.originalCommandLine || "No command-line arguments."}</code>
                {:else}
                  <p class="panel-empty">Original command line was not recorded in the build events.</p>
                {/if}
              </section>
              <section class="run-command-block">
                <h4>Canonical command line</h4>
                {#if bep?.canonicalCommandLine !== undefined}
                  <code class="run-command-code">{bep.canonicalCommandLine || "No command-line arguments."}</code>
                {:else}
                  <p class="panel-empty">Canonical command line was not recorded in the build events.</p>
                {/if}
              </section>
              <h3 class="run-arguments-heading">Bazel arguments</h3>
              {#if bep?.options}
                <div class="run-command-options">
                  <section class="run-command-block">
                    <h4>Explicit command options</h4>
                    <code class="run-command-code">{formatOptions(bep.options.explicitCmdLine) || "No command options."}</code>
                  </section>
                  <section class="run-command-block">
                    <h4>Effective command options</h4>
                    <code class="run-command-code">{formatOptions(bep.options.cmdLine) || "No command options."}</code>
                  </section>
                  {#if bep.options.explicitStartupOptions.length || bep.options.startupOptions.length}
                    <details class="run-startup-options">
                      <summary>Startup options</summary>
                      {#if bep.options.explicitStartupOptions.length}
                        <section class="run-command-block">
                          <h4>Explicit startup options</h4>
                          <code class="run-command-code">{formatOptions(bep.options.explicitStartupOptions)}</code>
                        </section>
                      {/if}
                      {#if bep.options.startupOptions.length}
                        <section class="run-command-block">
                          <h4>Effective startup options</h4>
                          <code class="run-command-code">{formatOptions(bep.options.startupOptions)}</code>
                        </section>
                      {/if}
                    </details>
                  {/if}
                </div>
              {:else if view.detailLoading}
                <p class="panel-empty">Loading arguments…</p>
              {:else if view.detail}
                <p class="run-notice">{sectionMessage(view.detail.bep.status === "ok" ? { status: "missing" } : view.detail.bep)}</p>
              {/if}
            {/if}
          {:else}
            <h2 class="panel-heading">Timing</h2>
            {#if running}<p class="panel-empty">
                Timing is available after the run finishes.
              </p>
            {:else if view.profileLoading}<p class="panel-empty" role="status">
                Loading profile…
              </p>
            {:else if view.profileError}<div class="run-notice error">
                Could not load timing data: {view.profileError}
                <button type="button" class="button" onclick={reloadProfile}>Retry</button>
              </div>
            {:else if view.profile}
              {#if view.profile.status !== "ok"}<p class="run-notice">
                  {sectionMessage(view.profile)}
                </p>{/if}
              {#if profile}{#key profile}<ProfileView {profile} />{/key}{/if}
            {/if}
          {/if}
        </section>
      {/if}
    {/each}
  </div>
</div>
