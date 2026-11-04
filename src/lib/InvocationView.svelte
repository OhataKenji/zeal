<script lang="ts">
  import InvocationHeader from "./InvocationHeader.svelte";
  import type { SessionView } from "./session";
  import {
    invocationStatus,
    sectionData,
    sectionMessage,
    sectionNoticeTone,
  } from "./invocation";
  import LogPanel from "./invocation/LogPanel.svelte";
  import Notice from "./Notice.svelte";
  import ProfileView from "./ProfileView.svelte";
  import TargetsPanel from "./invocation/TargetsPanel.svelte";
  import { formatOptions } from "./invocation/options";

  let { view, reload, reloadProfile }: {
    view: SessionView;
    reload: () => void;
    reloadProfile: () => void;
  } = $props();
  const selectedInvocation = $derived(view.selectedInvocation);
  const record = $derived(
    view.detail?.record ??
      view.invocations.find((inv) => inv.id === selectedInvocation),
  );
  const running = $derived(view.state?.activeInvocation === selectedInvocation);
  const profile = $derived(sectionData(view.profile ?? undefined));
  const bep = $derived(sectionData(view.detail?.bep));
  const status = $derived(
    invocationStatus(record, view.state?.activeInvocation ?? null, view.state?.stopping),
  );
  const sections = ["Targets", "Details", "Logs", "Timing"] as const;
  const tabs = ["All", ...sections] as const;
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
  <InvocationHeader {record} {status} fallback={selectedInvocation}>
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
  </InvocationHeader>

  {#if view.detailError}
    <Notice tone="error" onaction={reload}>Could not load invocation details: {view.detailError}</Notice>
  {/if}

  <div
    class="invocation-panel"
    bind:this={panel}
    class:invocation-all={active === "All"}
    id="invocation-panel-{active}"
    role="tabpanel"
    aria-labelledby="invocation-tab-{active}"
  >
    {#each sections as section}
      {#if active === "All" || active === section}
        <section class="invocation-section" aria-label={section}>
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
            <h2 class="panel-heading">Invocation details</h2>
            {#if record}
              <dl class="invocation-details">
                <dt>Command</dt>
                <dd>{#if record.cmd}<code>{record.cmd}</code>{:else}Not recorded{/if}</dd>
                <dt>Pattern</dt>
                <dd>{#if record.pattern}<code>{record.pattern}</code>{:else}Not recorded{/if}</dd>
                <dt>Workspace</dt>
                <dd><code>{record.workspace ?? "Not recorded"}</code></dd>
                <dt>Invocation ID</dt>
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
              <h3 class="command-arguments-heading">Options</h3>
              {#if bep?.options || bep?.residual?.length}
                <div class="invocation-command-options">
                  {#if bep?.options}
                    <section class="invocation-command-block">
                      <h4>Explicit command options</h4>
                      <code class="invocation-command-code">{formatOptions(bep.options.explicitCmdLine) || "No command options."}</code>
                    </section>
                    <section class="invocation-command-block">
                      <h4>Effective command options</h4>
                      <code class="invocation-command-code">{formatOptions(bep.options.cmdLine) || "No command options."}</code>
                    </section>
                  {/if}
                  {#if bep?.residual?.length}
                    <section class="invocation-command-block">
                      <h4>Residual</h4>
                      <code class="invocation-command-code">{formatOptions(bep.residual)}</code>
                    </section>
                  {/if}
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
                </div>
              {:else if view.detailLoading}
                <p class="panel-empty">Loading options…</p>
              {:else if view.detail}
                <Notice tone={sectionNoticeTone(view.detail.bep.status === "ok" ? { status: "missing" } : view.detail.bep)} announce={active !== "All"}
                  >{sectionMessage(view.detail.bep.status === "ok" ? { status: "missing" } : view.detail.bep)}</Notice
                >
              {/if}
              <h3 class="command-arguments-heading">Command lines</h3>
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
            {/if}
          {:else}
            <h2 class="panel-heading">Timing</h2>
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
