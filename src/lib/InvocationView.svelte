<script lang="ts">
  import InvocationHeader from "./InvocationHeader.svelte";
  import { canSend, type SessionView } from "./session";
  import {
    invocationStatus,
    rerunArgs,
    sectionData,
    sectionMessage,
    sectionNoticeTone,
    summarizeRunners,
  } from "./invocation";
  import ComparisonPanel from "./invocation/ComparisonPanel.svelte";
  import DiagnosticLog from "./invocation/DiagnosticLog.svelte";
  import LogPanel from "./invocation/LogPanel.svelte";
  import Notice from "./Notice.svelte";
  import ProfileView from "./ProfileView.svelte";
  import TargetsPanel from "./invocation/TargetsPanel.svelte";
  import { tabKeydown } from "./tabs";
  import { formatOptions } from "./invocation/options";
  import type { BazelCommand } from "./types";

  let { view, reload, reloadProfile, onstop, onrerun, onselecttarget }: {
    view: SessionView;
    reload: () => void;
    reloadProfile: () => void;
    onstop: (id: string) => void;
    onrerun?: (cmd: BazelCommand, pattern: string) => void;
    onselecttarget?: (label: string) => void;
  } = $props();
  const selected = $derived(view.invocation);
  const shown = $derived(selected ? (view.detail?.record ?? selected.record) : undefined);
  const execution = $derived(selected?.execution);
  const diagnostic = $derived(selected?.diagnostic ?? null);
  const profile = $derived(sectionData(view.profile ?? undefined));
  const bep = $derived(sectionData(view.detail?.bep));
  // Target links navigate only when the shown record belongs to the
  // workspace whose snapshot is loaded.
  const targetIndex = $derived(
    shown?.workspace && shown.workspace === view.state?.workspace && view.core?.bazel
      ? view.snapshot
      : null,
  );
  const runnerSummary = $derived(summarizeRunners(bep?.runnerCounts));
  const rerunCandidate = $derived(rerunArgs(shown));
  const canRerun = $derived(
    rerunCandidate !== null &&
      shown?.workspace === view.state?.workspace &&
      canSend(view, rerunCandidate.pattern),
  );
  const status = $derived(invocationStatus(shown, execution));
  const sections = ["Targets", "Details", "Logs", "Timing"] as const;
  const tabs = ["All", ...sections, "Compare"] as const;
  let active = $state<(typeof tabs)[number]>("All");
  let panel = $state<HTMLDivElement>();

  $effect(() => {
    void active;
    if (panel) panel.scrollTop = 0;
  });
</script>

{#if diagnostic && shown}
  <div class="invocation-page">
    <InvocationHeader record={shown} {status} {canRerun} {onstop} {onrerun} />
    <Notice tone="neutral">
      No build event record. Bazel process exit code: {diagnostic.processExitCode ??
        "unknown"}.
    </Notice>
    {#key `${diagnostic.workspace}:${diagnostic.id}`}
      <DiagnosticLog {diagnostic} />
    {/key}
  </div>
{:else if shown}
  <div class="invocation-page">
    <InvocationHeader record={shown} {status} {execution} {canRerun} {onstop} {onrerun}>
      <div class="tabs" role="tablist" aria-label="Invocation views">
        {#each tabs as tab}
          <button
            type="button"
            id="invocation-tab-{tab}"
            role="tab"
            aria-selected={active === tab}
            aria-controls={active === tab ? `invocation-panel-${tab}` : undefined}
            tabindex={active === tab ? 0 : -1}
            onclick={() => (active = tab)}
            onkeydown={tabKeydown}
          >{tab}</button>
        {/each}
        {#if view.detailLoading}<span class="panel-loading" role="status"
            >Loading invocation…</span
          >{/if}
      </div>
    </InvocationHeader>

    {#if view.detailError}
      <Notice tone="error" onaction={reload}>Could not load invocation details: {view.detailError}</Notice>
    {/if}

    <div
      class="invocation-panel"
      bind:this={panel}
      class:invocation-all={active === "All"}
      class:invocation-timing={active === "Timing"}
      id="invocation-panel-{active}"
      role="tabpanel"
      aria-labelledby="invocation-tab-{active}"
    >
      {#if active === "Compare"}
        {#key `${view.state?.workspace ?? ""}:${shown.id}`}
          <ComparisonPanel
            left={view.detail?.record.id === shown.id ? view.detail : null}
            leftLoading={view.detailLoading}
            leftError={view.detailError}
            workspace={view.state?.workspace ?? ""}
            runningIds={view.core?.invocations.map((entry) => entry.id) ?? []}
          />
        {/key}
      {/if}
      {#each sections as section}
        {#if active === "All" || active === section}
          <section class="invocation-section" aria-labelledby={active === "All" ? `invocation-heading-${section}` : `invocation-tab-${section}`}>
            {#if active === "All"}<h2 class="panel-heading" id="invocation-heading-{section}">{section}</h2>{/if}
            {#if section === "Targets"}
              <TargetsPanel section={view.detail?.bep} index={targetIndex} {onselecttarget} />
            {:else if section === "Logs"}
              <LogPanel section={view.detail?.log} loading={view.detailLoading} />
            {:else if section === "Details"}
              {#if runnerSummary}
                <div class="cache-summary">
                  <div class="cache-summary-head">
                    <h3 class="panel-subheading">Spawn Cache Hits (BEP)</h3>
                    <span class="mono">{runnerSummary.cached} / {runnerSummary.total} cached ({runnerSummary.hitPct}%)</span>
                  </div>
                  <div class="cache-bar" aria-hidden="true">
                    <div class="cache-bar-hit" style:width={`${runnerSummary.hitPct}%`}></div>
                    <div class="cache-bar-exec" style:width={`${100 - runnerSummary.hitPct}%`}></div>
                  </div>
                  <div class="cache-legend">
                    {#each runnerSummary.runners as r (r.name)}
                      <span class="mono">
                        <i class="cache-sw" class:hit={r.cached}></i>{r.name}: {r.count}
                      </span>
                    {/each}
                  </div>
                </div>
              {/if}
              <dl class="invocation-details">
                <dt>Command</dt>
                <dd>{#if shown.cmd}<code>{shown.cmd}</code>{:else}Not recorded{/if}</dd>
                <dt>Pattern</dt>
                <dd>{#if shown.pattern}<code>{shown.pattern}</code>{:else}Not recorded{/if}</dd>
                <dt>Workspace</dt>
                <dd><code>{shown.workspace ?? "Not recorded"}</code></dd>
                <dt>Invocation ID</dt>
                <dd><code>{shown.id}</code></dd>
                <dt>Started</dt>
                <dd>{shown.startedAt === undefined ? "Not recorded" : new Date(shown.startedAt).toLocaleString()}</dd>
                <dt>Finished</dt>
                <dd>
                  {shown.finishedAt === undefined
                    ? "Not recorded"
                    : new Date(shown.finishedAt).toLocaleString()}
                </dd>
                <dt>Exit code</dt>
                <dd>{shown.exitCode ?? "Unknown"}</dd>
                {#if shown.startedAt !== undefined && shown.finishedAt !== undefined}<dt>Wall time</dt>
                  <dd>
                    {((shown.finishedAt - shown.startedAt) / 1000).toFixed(3)}s
                  </dd>{/if}
              </dl>
              <h3 class="panel-subheading">Options</h3>
              {#if bep?.options}
                <section class="invocation-command-block">
                  <h4>Explicit command options</h4>
                  <code class="invocation-command-code">{formatOptions(bep.options.explicitCmdLine) || "No command options."}</code>
                </section>
                <section class="invocation-command-block">
                  <h4>Effective command options</h4>
                  <code class="invocation-command-code">{formatOptions(bep.options.cmdLine) || "No command options."}</code>
                </section>
              {:else if view.detailLoading}
                <p class="panel-empty">Loading options…</p>
              {:else if view.detail}
                <Notice tone={sectionNoticeTone(view.detail.bep.status === "ok" ? { status: "missing" } : view.detail.bep)} announce={active !== "All"}
                  >{sectionMessage(view.detail.bep.status === "ok" ? { status: "missing" } : view.detail.bep)}</Notice
                >
              {/if}
              <h3 class="panel-subheading">Command lines</h3>
              <details class="invocation-collapsible">
                <summary>Canonical command line</summary>
                {#if bep?.canonicalCommandLine !== undefined}
                  <code class="invocation-command-code">{bep.canonicalCommandLine || "No command-line arguments."}</code>
                {:else}
                  <p class="panel-empty">Canonical command line was not recorded in the build events.</p>
                {/if}
              </details>
              <details class="invocation-collapsible">
                <summary>Original command line</summary>
                {#if bep?.originalCommandLine !== undefined}
                  <code class="invocation-command-code">{bep.originalCommandLine || "No command-line arguments."}</code>
                {:else}
                  <p class="panel-empty">Original command line was not recorded in the build events.</p>
                {/if}
              </details>
              {#if bep?.options && (bep.options.explicitStartupOptions.length || bep.options.startupOptions.length)}
                <details class="invocation-collapsible">
                  <summary>Startup options</summary>
                  {#if bep.options.explicitStartupOptions.length}
                    <section class="invocation-command-block">
                      <h4>Explicit startup options</h4>
                      <code class="invocation-command-code">{formatOptions(bep.options.explicitStartupOptions)}</code>
                    </section>
                  {/if}
                  {#if bep.options.startupOptions.length}
                    <section class="invocation-command-block">
                      <h4>Effective startup options</h4>
                      <code class="invocation-command-code">{formatOptions(bep.options.startupOptions)}</code>
                    </section>
                  {/if}
                </details>
              {/if}
            {:else}
              {#if execution}<p class="panel-empty">
                  Timing is available after the invocation finishes.
                </p>
              {:else if view.profileLoading}<p class="panel-empty" role="status">
                  Loading profile…
                </p>
              {:else if view.profileError}<Notice tone="error" onaction={reloadProfile}
                  >Could not load timing data: {view.profileError}</Notice
                >
              {:else if view.profile}
                {#if view.profile.status !== "ok"}<Notice tone={sectionNoticeTone(view.profile)}
                    >{sectionMessage(view.profile)}</Notice
                  >{/if}
                {#if profile}{#key profile}<ProfileView {profile} />{/key}{/if}
              {/if}
            {/if}
          </section>
        {/if}
      {/each}
    </div>
  </div>
{:else if view.historyLoading}
  <p class="panel-empty" role="status">Loading invocation…</p>
{:else if view.invocationsError}
  <Notice tone="error">{view.invocationsError}</Notice>
{:else}
  <Notice tone="neutral">
    No invocation record found for <code>{view.selectedInvocationId}</code>. It may be older than
    the retained history, or the command may have exited before sending build events.
  </Notice>
{/if}
