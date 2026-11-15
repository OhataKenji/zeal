<script lang="ts">
  import InvocationHeader from "./InvocationHeader.svelte";
  import type { InvocationRecord } from "./types";
  import type { SessionView } from "./session";
  import {
    invocationStatus,
    sectionData,
    sectionMessage,
    sectionNoticeTone,
  } from "./invocation";
  import ComparisonPanel from "./invocation/ComparisonPanel.svelte";
  import LogPanel from "./invocation/LogPanel.svelte";
  import Notice from "./Notice.svelte";
  import ProfileView from "./ProfileView.svelte";
  import TargetsPanel from "./invocation/TargetsPanel.svelte";
  import { formatOptions } from "./invocation/options";

  let { view, reload, reloadProfile, onstop, onconsole }: {
    view: SessionView;
    reload: () => void;
    reloadProfile: () => void;
    onstop: (id: string) => void;
    onconsole: (id: string) => void;
  } = $props();
  const selectedInvocation = $derived(view.content.kind === "invocation" ? view.content.id : null);
  const record = $derived(
    view.detail?.record ??
      view.invocations.find((inv) => inv.id === selectedInvocation),
  );
  const execution = $derived(view.execution);
  const shown = $derived<InvocationRecord | undefined>(record ?? execution ?? undefined);
  const running = $derived(execution?.running === true);
  const profile = $derived(sectionData(view.profile ?? undefined));
  const bep = $derived(sectionData(view.detail?.bep));
  const status = $derived(invocationStatus(record, execution ?? undefined));
  const sections = ["Targets", "Details", "Logs", "Timing"] as const;
  const tabs = ["All", ...sections, "Compare"] as const;
  let active = $state<(typeof tabs)[number]>("All");
  let panel = $state<HTMLDivElement>();

  $effect(() => {
    void active;
    void selectedInvocation;
    if (panel) panel.scrollTop = 0;
  });

  function nextTabIndex(key: string, index: number): number {
    switch (key) {
      case "ArrowRight":
        return (index + 1) % tabs.length;
      case "ArrowLeft":
        return (index + tabs.length - 1) % tabs.length;
      case "Home":
        return 0;
      case "End":
        return tabs.length - 1;
      default:
        return -1;
    }
  }

  function tabKey(event: KeyboardEvent, index: number) {
    const next = nextTabIndex(event.key, index);
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

<div class="invocation-page">
  <InvocationHeader record={shown} {status} fallback={selectedInvocation ?? execution?.id}>
    <div class="invocation-tabs" role="tablist" aria-label="Invocation views">
      {#each tabs as tab, index}
        <button
          type="button"
          class="button button-ghost tab-button"
          id="invocation-tab-{tab}"
          role="tab"
          aria-selected={active === tab}
          aria-controls="invocation-panel-{tab}"
          tabindex={active === tab ? 0 : -1}
          onclick={() => (active = tab)}
          onkeydown={(event) => tabKey(event, index)}
        >{tab}</button>
      {/each}
      {#if view.detailLoading}<span class="panel-loading" role="status"
          >Loading invocation…</span
        >{/if}
    </div>
    {#if execution}
      <div class="invocation-actions">
        {#if execution.console}
          <button
            type="button"
            class="button button-ghost"
            onclick={() => onconsole(execution.id)}>Console</button
          >
        {/if}
        {#if execution.stoppable}
          <button
            type="button"
            class="button button-ghost"
            disabled={execution.stopping}
            onclick={() => onstop(execution.id)}
            >{execution.stopping ? "Stopping…" : "Stop"}</button
          >
        {/if}
      </div>
      {#if execution.note}<Notice tone="warning">{execution.note}</Notice>{/if}
    {/if}
  </InvocationHeader>

  {#if view.detailError}
    <Notice tone="error" onaction={reload}>Could not load invocation details: {view.detailError}</Notice>
  {/if}
  {#if !shown && !view.detailLoading}
    <Notice tone="neutral">No local process or saved record is available.</Notice>
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
      {#key `${view.state?.workspace ?? ""}:${selectedInvocation ?? ""}`}
        <ComparisonPanel
          left={view.detail?.record.id === selectedInvocation ? view.detail : null}
          leftLoading={view.detailLoading}
          leftError={view.detailError}
          workspace={view.state?.workspace ?? ""}
          runningIds={view.core?.invocations.filter((entry) => entry.running).map((entry) => entry.id) ?? []}
        />
      {/key}
    {/if}
    {#each sections as section}
      {#if active === "All" || active === section}
        <section class="invocation-section" aria-labelledby={active === "All" ? `invocation-heading-${section}` : `invocation-tab-${section}`}>
          {#if active === "All"}<h2 class="panel-heading" id="invocation-heading-{section}">{section}</h2>{/if}
          {#if section === "Targets"}
            <TargetsPanel section={view.detail?.bep} />
          {:else if section === "Logs"}
            {#key selectedInvocation}
            <LogPanel
              section={view.detail?.log}
              loading={view.detailLoading}
            />
            {/key}
          {:else if section === "Details"}
            {#if shown}
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
                {#if !record && execution?.processExitCode != null}
                  <dt>Process</dt>
                  <dd>Exited (code {execution.processExitCode})</dd>
                {/if}
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
            {/if}
          {:else}
            {#if running}<p class="panel-empty">
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
