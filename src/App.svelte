<script lang="ts">
  import { ArrowClockwise, Gear } from "phosphor-svelte";
  import { onMount } from "svelte";
  import { app, connect, getSession, refresh } from "./lib/state.svelte";
  import {
    bazelBusyReason,
    canSend,
    canStop,
    invocationId,
    sessionBusy,
    type MainPage,
  } from "./lib/session/session";
  import { createIndex } from "./lib/search";
  import { targetError } from "./lib/target";
  import PackageView from "./lib/PackageView.svelte";
  import TargetView from "./lib/TargetView.svelte";
  import WorkspacePrompt from "./lib/WorkspacePrompt.svelte";
  import Notice from "./lib/Notice.svelte";
  import InvocationView from "./lib/InvocationView.svelte";
  import AnalyticsView from "./lib/AnalyticsView.svelte";
  import QueryView from "./lib/query/QueryView.svelte";
  import CommandComposer from "./lib/workbench/CommandComposer.svelte";
  import ExplorerSidebar from "./lib/workbench/ExplorerSidebar.svelte";
  import type { AppState, ExplorerSelection } from "./lib/types";

  let consoleComponent: Promise<typeof import("./lib/ConsoleView.svelte")> | undefined;
  function loadConsole() {
    return consoleComponent ??= import("./lib/ConsoleView.svelte");
  }

  const initialPage: MainPage = (() => {
    if (typeof location === "undefined") return "explorer";
    const view = new URLSearchParams(location.search).get("view");
    return view === "query" || view === "analytics" ? view : "explorer";
  })();

  onMount(() => connect(initialPage));

  const st = $derived(app.state);
  const snap = $derived(app.view.targets.snapshot);
  const page = $derived(app.view.page);
  const selectedInvocation = $derived(invocationId(page));
  const selection = $derived(app.view.targets.selection);
  const selected = $derived(selection?.kind === "target" ? selection.label : null);

  function retryTargets() {
    if (app.view.targets.snapshotError) void getSession().loadSnapshot();
    else refresh();
  }

  let revealRequest = $state(0);
  let targetField = $state("");
  let targetsVisible = $state(true);
  let switchingWs = $state(false);
  const busy = $derived(sessionBusy(app.view));
  // Queries serialize with other Bazel commands and ingest in the core; the reason
  // text doubles as the disabled-Execute explanation.
  const queryBusyReason = $derived.by((): string | null => {
    if (!app.view.ready) return "not connected to Bazel";
    if (app.view.opening) return "the workspace is changing";
    if (app.view.starting) return "an invocation is starting";
    return bazelBusyReason(st);
  });
  const running = $derived(!!st?.activeInvocation);
  const inputError = $derived(
    targetField === "" ? null : targetError(targetField),
  );

  // The composer is the only search entry point. Typing opens the dropdown;
  // programmatic insertion (tree, relations, invocation rows) only replaces the
  // field. Picking a candidate reveals it in the target tree.
  const index = $derived(snap ? createIndex(snap.targets) : null);

  function pickTarget(value: string) {
    select({ kind: "target", label: value });
    revealRequest += 1;
  }

  function select(selection: ExplorerSelection) {
    getSession().select(selection);
    if (selection?.kind === "target") targetField = selection.label;
  }
</script>

{#snippet workspaceContent(state: AppState, workspace: string)}
  <main
    class="content"
    class:invocation-content={page.kind !== "explorer"}
  >
    {#if app.view.console}
      {#key app.view.console.id}
        <div class="console-container" hidden={page.kind !== "console"}>
          {#await loadConsole() then { default: ConsoleView }}
          <ConsoleView record={app.view.invocations.records.find((inv) => inv.id === app.view.console?.id) ?? app.view.console}
            visible={page.kind === "console"} running={state.activeInvocation === app.view.console.id} stopping={state.stopping} />
          {/await}
        </div>
      {/key}
    {/if}
    {#if page.kind !== "console"}
    {#if snap?.meta.status === "partial"}
      <Notice tone="warning">
        Skipped some packages (exit {state.lastError?.exitCode ?? "?"}) —
        partial results from
        {new Date(snap.meta.fetchedAt).toLocaleTimeString()}
      </Notice>
    {:else if snap && (state.lastError || app.view.targets.snapshotError)}
      <Notice tone="warning" onaction={app.view.targets.snapshotError ? getSession().loadSnapshot : undefined}>
        Refresh failed — showing the snapshot from {new Date(
          snap.meta.fetchedAt,
        ).toLocaleTimeString()} ({state.lastError?.summary ?? app.view.targets.snapshotError})
      </Notice>
    {/if}
    {#if state.ingest === "running" || app.view.targets.snapshotLoading}
      <p class="dim">Loading targets…</p>
    {/if}
    {#if selectedInvocation}
      {#key selectedInvocation}
        <InvocationView
          id={selectedInvocation}
          record={app.view.invocations.detail?.record ??
            app.view.invocations.records.find((inv) => inv.id === selectedInvocation)}
          view={app.view.invocations}
          running={state.activeInvocation === selectedInvocation}
          stopping={state.stopping}
          reload={getSession().loadDetail}
          reloadProfile={getSession().loadProfile}
        />
      {/key}
    {:else if page.kind === "analytics"}
      {#key workspace}<AnalyticsView {workspace} invocations={app.view.invocations.records} />{/key}
    {:else if page.kind === "query"}
      {#key workspace}
        <QueryView {snap} busyReason={queryBusyReason} />
      {/key}
    {:else if snap && selection?.kind === "package"}
      <PackageView index={snap} pkg={selection.path} onselect={(label) => select({ kind: "target", label })} />
    {:else if snap && selected}
      {#if app.view.targets.targetDetail?.target.label === selected}
        {#if app.view.targets.targetDetail.status === "partial"}
          <Notice tone="warning">Some target details could not be loaded.</Notice>
        {/if}
        {#key `${selected}:${snap.meta.fetchedAt}`}
          <TargetView
            index={snap}
            detail={app.view.targets.targetDetail}
            wsRoot={workspace}
            onselect={(label) => select({ kind: "target", label })}
          />
        {/key}
      {:else if app.view.targets.targetDetailError}
        <Notice tone="error" onaction={() =>
          app.view.targets.snapshotError
            ? void getSession().loadSnapshot()
            : getSession().select(selection)}>
          Could not load target details: {app.view.targets.targetDetailError}
        </Notice>
      {:else}
        <p class="dim">
          {st?.ingest === "running" || st?.query === "running" || st?.activeInvocation
            ? "Waiting for Bazel…"
            : "Loading target details…"}
        </p>
      {/if}
    {:else if snap}
      <p class="dim">Select a target in the tree</p>
    {:else if state.ingest !== "running" && !app.view.targets.snapshotLoading && (app.view.targets.snapshotError || state.lastError)}
      <Notice tone="error" announce={!targetsVisible} onaction={retryTargets}>
        {#if app.view.targets.snapshotError}
          {app.view.targets.snapshotError}
        {:else}
          Failed to load targets: {state.lastError?.summary}
        {/if}
      </Notice>
    {/if}
    {/if}
  </main>
{/snippet}

<div class="win">
  <div class="toolbar">
    <div class="workspace-picker">
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="76 76 360 360"
        width="22"
        height="22"
        class="brand-icon zeal-logo"
        aria-hidden="true"
      >
        <polygon points="84,104 428,104 428,216 276,216 164,304 84,304" fill="#34d399" />
        <polygon points="428,208 348,208 236,296 84,296 84,408 428,408" fill="#0891b2" />
      </svg>
      {#if st?.workspace}
        <span class="label" title={st.workspace}
          >{st.workspace.split(/[\\/]/).filter(Boolean).at(-1) ??
            st.workspace}</span
        >
        <div class="workspace-actions">
          <button
            type="button"
            class="button button-ghost icon-button"
            aria-label="Switch workspace"
            aria-expanded={switchingWs}
            title="Switch workspace"
            disabled={busy}
            onclick={() => (switchingWs = !switchingWs)}
          >
            <Gear size={14} />
          </button>
          <button
            type="button"
            class="button button-ghost icon-button"
            aria-label="Reload targets"
            title="Reload targets"
            onclick={refresh}
            disabled={busy}
          >
            <ArrowClockwise size={14} />
          </button>
        </div>
      {:else}
        <strong>Zeal</strong>
      {/if}
    </div>
    <CommandComposer
      {index}
      value={targetField}
      {inputError}
      {running}
      disabled={switchingWs ||
        (running ? !canStop(app.view) : !canSend(app.view, targetField))}
      oninput={(v) => (targetField = v)}
      onpick={pickTarget}
      onstart={(command, target, argumentsText) =>
        void getSession().start(command, target, argumentsText)}
      onstop={() => void getSession().stop()}
    />
  </div>
  {#if app.view.error}<Notice tone="error">{app.view.error}</Notice>{/if}
  {#if st?.invocationNote}<Notice tone="error">{st.invocationNote}</Notice>{/if}
  {#if st?.besError}<Notice tone={st.besReady ? "warning" : "error"}
      >BES receiver: {st.besError}</Notice
    >{/if}
  {#if inputError}<p class="field-error composer-error" id="composer-target-error" role="status">{inputError}</p>{/if}

  {#if st === null}
    <div class="content dim">loading…</div>
  {:else}
    {#if st.workspace === null || switchingWs}
      <div class="content">
        <WorkspacePrompt
          initial={st.workspace ?? ""}
          initialScope={st.scope}
          onclose={st.workspace ? () => (switchingWs = false) : undefined}
        />
        {#if st.workspace && st.bazel === null}
          <Notice tone="error">bazelisk / bazel not found on PATH</Notice>
        {/if}
      </div>
    {/if}
    <div
      class="win-body"
      style:display={st.workspace === null || switchingWs ? "none" : undefined}
    >
      <ExplorerSidebar
        visible={st.workspace !== null && !switchingWs}
        state={st}
        targets={app.view.targets}
        history={{
          records: app.view.invocations.records,
          error: app.view.invocations.error,
          selectedId: selectedInvocation,
          consoleId: page.kind === "console" ? (app.view.console?.id ?? null) : null,
        }}
        page={page.kind}
        {revealRequest}
        onselect={select}
        oninvocation={(inv) =>
          inv.id === st?.activeInvocation
            ? getSession().openConsole(inv)
            : getSession().selectInvocation(inv.id)}
        onpage={(p) => getSession().showPage(p)}
        onretryTargets={retryTargets}
        onretryInvocations={getSession().loadInvocations}
        onTargetVisibility={(v) => (targetsVisible = v)}
      />
      {#if st.workspace !== null && !switchingWs}
        {@render workspaceContent(st, st.workspace)}
      {/if}
    </div>
  {/if}
</div>
