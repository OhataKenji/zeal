<script lang="ts">
  import {
    CaretDown,
    CaretRight,
    ChartBar,
    ArrowClockwise,
    Gear,
    Play,
    Stop,
    TreeStructure,
  } from "phosphor-svelte";
  import { onMount, tick } from "svelte";
  import { app, connect, getSession, refresh } from "./lib/state.svelte";
  import { canSend, canStop, sessionBusy } from "./lib/session";
  import { invocationStatus } from "./lib/invocation";
  import { createIndex } from "./lib/search";
  import { targetError } from "./lib/target";
  import { buildTree } from "./lib/model";
  import Tree from "./lib/Tree.svelte";
  import PackageView from "./lib/PackageView.svelte";
  import TargetView from "./lib/TargetView.svelte";
  import WorkspacePrompt from "./lib/WorkspacePrompt.svelte";
  import InvocationStatus from "./lib/InvocationStatus.svelte";
  import Notice from "./lib/Notice.svelte";
  import InvocationView from "./lib/InvocationView.svelte";
  import AnalyticsView from "./lib/AnalyticsView.svelte";
  import QueryView from "./lib/QueryView.svelte";
  import SearchInput from "./lib/SearchInput.svelte";
  import type { AppState, BazelCommand, ExplorerSelection } from "./lib/types";

  let consoleComponent: Promise<typeof import("./lib/ConsoleView.svelte")> | undefined;
  function loadConsole() {
    return consoleComponent ??= import("./lib/ConsoleView.svelte");
  }

  const requestedPage = typeof location === "undefined"
    ? null
    : new URLSearchParams(location.search).get("view");
  const initialPage = requestedPage === "analytics" || requestedPage === "query"
    ? requestedPage
    : "targets";
  onMount(() => connect(initialPage));

  const st = $derived(app.state);
  const snap = $derived(app.view.snapshot);
  const tree = $derived(snap ? buildTree(snap) : null);
  const selection = $derived(app.view.selection);
  const selected = $derived(selection?.kind === "target" ? selection.label : null);
  const content = $derived(app.view.content);

  function retryTargets() {
    if (app.view.snapshotError) void getSession().loadSnapshot();
    else refresh();
  }

  let revealRequest = $state(0);
  let targetField = $state("");
  let cmd = $state<BazelCommand>("build");
  let argumentsText = $state("");
  let argumentsOpen = $state(false);
  let argumentsToggle: HTMLButtonElement | undefined;
  let argumentsPanel = $state<HTMLDivElement>();
  let argumentsInput = $state<HTMLInputElement>();
  const argumentsConfigured = $derived(argumentsText.trim() !== "");
  const switchingWs = $derived(app.view.workspacePromptOpen);
  const busy = $derived(sessionBusy(app.view));
  const running = $derived(!!st?.activeInvocation);
  const actionLabel = $derived(
    running ? "Stop" : cmd === "test" ? "Test" : cmd === "run" ? "Run" : "Build",
  );
  const inputError = $derived(
    targetField === "" ? null : targetError(targetField),
  );

  // The composer is the only search entry point. Typing opens the dropdown;
  // programmatic insertion (tree, relations, invocation rows) only replaces the
  // field. Picking a candidate reveals it in the target tree.
  const index = $derived(snap ? createIndex(snap.targets) : null);

  function pickTarget(value: string) {
    select({ kind: "target", label: value });
    collapsed.targets = false;
    revealRequest += 1;
  }

  function submit() {
    if (running) {
      getSession().stop();
    } else {
      getSession().start(cmd, targetField, argumentsText);
    }
  }

  async function setArgumentsOpen(open: boolean) {
    argumentsOpen = open;
    await tick();
    if (argumentsOpen === open) {
      (open ? argumentsInput : argumentsToggle)?.focus();
    }
  }

  function argumentsKey(event: KeyboardEvent) {
    if (!argumentsOpen || event.isComposing || event.key !== "Escape") return;
    event.preventDefault();
    void setArgumentsOpen(false);
  }

  function dismissArguments(event: Event) {
    if (
      argumentsOpen &&
      event.target instanceof Node &&
      !argumentsPanel?.contains(event.target) &&
      !argumentsToggle?.contains(event.target)
    ) {
      argumentsOpen = false;
    }
  }

  // Sidebar split: share of height given to the targets pane, and per-pane
  // collapse. The divider drags the boundary like VS Code's stacked views.
  let targetsShare = $state(0.55);
  let collapsed = $state({ targets: false, invocations: false });

  function dividerDown(e: PointerEvent) {
    e.preventDefault();
    const divider = e.currentTarget;
    if (!(divider instanceof HTMLElement) || !divider.parentElement) return;
    const sidebar = divider.parentElement;
    const move = (ev: PointerEvent) => {
      const r = sidebar.getBoundingClientRect();
      targetsShare = Math.min(
        0.85,
        Math.max(0.15, (ev.clientY - r.top) / r.height),
      );
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function select(selection: ExplorerSelection) {
    getSession().select(selection);
    if (selection?.kind === "target") targetField = selection.label;
  }
</script>

{#snippet targetPane(state: AppState)}
  <section
    class="pane"
    class:collapsed={collapsed.targets}
    style:flex-grow={collapsed.targets ? 0 : targetsShare}
  >
    <div class="pane-heading">
      <button
        class="pane-head"
        aria-expanded={!collapsed.targets}
        onclick={() => (collapsed.targets = !collapsed.targets)}
      >
        {#if collapsed.targets}<CaretRight size={10} />{:else}<CaretDown
            size={10}
          />{/if} Targets
        <span class="pane-count"
          >{snap?.targets.length.toLocaleString() ?? "—"}</span
        >
      </button>
      <button
        class="analytics-link"
        class:active={content.kind === "query"}
        onclick={() => getSession().showPage("query")}><TreeStructure size={12} /> Query</button
      >
    </div>
    {#if !collapsed.targets}
      <div class="pane-body">
        {#if snap && tree}
          <Tree root={tree} {selection} {revealRequest} onselect={select} />
        {:else if state.ingest === "running" || app.view.snapshotLoading}
          <p class="dim">Loading targets…</p>
        {:else if app.view.snapshotError || state.lastError}
          <Notice tone="error" onaction={retryTargets}
            >{app.view.snapshotError ?? state.lastError?.summary}</Notice
          >
        {:else}
          <p class="dim">—</p>
        {/if}
      </div>
    {/if}
  </section>
{/snippet}

{#snippet invocationPane(state: AppState)}
  <section
    class="pane"
    class:collapsed={collapsed.invocations}
    style:flex-grow={collapsed.invocations ? 0 : 1 - targetsShare}
  >
    <div class="pane-heading">
      <button
        class="pane-head"
        aria-expanded={!collapsed.invocations}
        onclick={() => (collapsed.invocations = !collapsed.invocations)}
      >
        {#if collapsed.invocations}<CaretRight size={10} />{:else}<CaretDown
            size={10}
          />{/if} Invocations
      </button>
      <button
        class="analytics-link"
        class:active={content.kind === "analytics"}
        onclick={() => getSession().showPage("analytics")}><ChartBar size={12} /> Analytics</button
      >
    </div>
    {#if !collapsed.invocations}
      <div class="pane-body">
        <ul class="list">
          {#each app.view.invocations as inv (inv.id)}
            {@const status = invocationStatus(inv, state.activeInvocation, state.stopping)}
            {@const active =
              (content.kind === "invocation" && content.id === inv.id) ||
              (content.kind === "console" && app.view.console?.id === inv.id)}
            <li>
              <button
                class="invocation-row"
                title={`${inv.cmd ?? "Command not recorded"} ${inv.pattern ?? inv.id} — ${status.label}`}
                aria-label={`${inv.cmd ?? "Command not recorded"} ${inv.pattern ?? inv.id}: ${status.label}`}
                class:selected={active}
                aria-current={active ? "true" : undefined}
                onclick={() =>
                  inv.id === state.activeInvocation
                    ? getSession().openConsole(inv)
                    : getSession().selectInvocation(inv.id)}
              >
                <span class="invocation-label"
                  ><InvocationStatus {...status} compact /><span class="invocation-command"
                    >{inv.cmd ?? "?"}</span
                  >
                  {#if inv.startedAt !== undefined}
                  <time datetime={new Date(inv.startedAt).toISOString()}
                    >{new Date(inv.startedAt).toLocaleString([], {
                      year: "numeric",
                      month: "2-digit",
                      day: "2-digit",
                      hour: "2-digit",
                      minute: "2-digit",
                    })}</time
                  >{:else}<span>Time not recorded</span>{/if}</span
                >
                <span class="invocation-pattern">{inv.pattern ?? inv.id}</span>
              </button>
            </li>
          {/each}
        </ul>
        {#if app.view.invocationsError}<Notice tone="error" onaction={getSession().loadInvocations}
            >{app.view.invocationsError}</Notice
          >
        {/if}
      </div>
    {/if}
  </section>
{/snippet}

{#snippet workspaceContent(state: AppState, workspace: string)}
  <main
    class="content"
    class:invocation-content={content.kind !== "targets"}
  >
    {#if app.view.console}
      {#key app.view.console.id}
        <div class="console-container" hidden={content.kind !== "console"}>
          {#await loadConsole() then { default: ConsoleView }}
          <ConsoleView record={app.view.invocations.find((inv) => inv.id === app.view.console?.id) ?? app.view.console}
            visible={content.kind === "console"} running={state.activeInvocation === app.view.console.id} stopping={state.stopping} />
          {/await}
        </div>
      {/key}
    {/if}
    {#if content.kind !== "console"}
    {#if snap?.meta.status === "partial"}
      <Notice tone="warning">
        Skipped some packages (exit {state.lastError?.exitCode ?? "?"}) —
        partial results from
        {new Date(snap.meta.fetchedAt).toLocaleTimeString()}
      </Notice>
    {:else if snap && (state.lastError || app.view.snapshotError)}
      <Notice tone="warning" onaction={app.view.snapshotError ? getSession().loadSnapshot : undefined}>
        Refresh failed — showing the snapshot from {new Date(
          snap.meta.fetchedAt,
        ).toLocaleTimeString()} ({state.lastError?.summary ?? app.view.snapshotError})
      </Notice>
    {/if}
    {#if state.ingest === "running" || app.view.snapshotLoading}
      <p class="dim">Loading targets…</p>
    {/if}
    {#if content.kind === "invocation"}
      {#key content.id}
        <InvocationView view={app.view} reload={getSession().loadDetail} reloadProfile={getSession().loadProfile} />
      {/key}
    {:else if content.kind === "analytics"}
      {#key workspace}<AnalyticsView {workspace} invocations={app.view.invocations} />{/key}
    {:else if content.kind === "query"}
      {#key workspace}
        <QueryView {snap} />
      {/key}
    {:else if snap && selection?.kind === "package"}
      <PackageView index={snap} pkg={selection.path} onselect={(label) => select({ kind: "target", label })} />
    {:else if snap && selected}
      {#if app.view.targetDetail?.target.label === selected}
        {#if app.view.targetDetail.status === "partial"}
          <Notice tone="warning">Some target details could not be loaded.</Notice>
        {/if}
        {#key `${selected}:${snap.meta.fetchedAt}`}
          <TargetView
            index={snap}
            detail={app.view.targetDetail}
            outputs={app.view.targetOutputs}
            outputsError={app.view.targetOutputsError}
            actions={app.view.targetActions}
            actionsError={app.view.targetActionsError}
            wsRoot={workspace}
            onselect={(label) => select({ kind: "target", label })}
          />
        {/key}
      {:else if app.view.targetDetailError}
        <Notice tone="error" onaction={() =>
          app.view.snapshotError
            ? void getSession().loadSnapshot()
            : getSession().select(selection)}>
          Could not load target details: {app.view.targetDetailError}
        </Notice>
      {:else}
        <p class="dim">
          {st?.ingest === "running" || st?.activeInvocation
            ? "Waiting for Bazel…"
            : "Loading target details…"}
        </p>
      {/if}
    {:else if snap}
      <p class="dim">Select a target in the tree</p>
    {:else if state.ingest !== "running" && !app.view.snapshotLoading && (app.view.snapshotError || state.lastError)}
      <Notice tone="error" announce={collapsed.targets} onaction={retryTargets}>
        {#if app.view.snapshotError}
          {app.view.snapshotError}
        {:else}
          Failed to load targets: {state.lastError?.summary}
        {/if}
      </Notice>
    {/if}
    {/if}
  </main>
{/snippet}

<svelte:window
  onpointerdown={dismissArguments}
  onfocusin={dismissArguments}
  onkeydown={argumentsKey}
/>

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
            onclick={() => getSession().setWorkspacePromptOpen(!switchingWs)}
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
    <div class="composer" class:composer-invalid={!!inputError}>
      <select class="select" aria-label="Bazel command" bind:value={cmd}>
        <option>build</option>
        <option>test</option>
        <option>run</option>
      </select>
      <div class="composer-search">
        <SearchInput
          class="input composer-target"
          aria-label="Target label or pattern"
          aria-invalid={!!inputError}
          aria-describedby={inputError ? "composer-target-error" : undefined}
          placeholder="Fuzzy :all / //pkg:target"
          {index}
          value={targetField}
          oninput={(v) => (targetField = v)}
          onpick={pickTarget}
        />
      </div>
      <button
        bind:this={argumentsToggle}
        type="button"
        class="button button-ghost arguments-toggle"
        aria-label={argumentsConfigured ? "Arguments (configured)" : "Arguments"}
        aria-expanded={argumentsOpen}
        aria-controls="command-arguments-popover"
        title={argumentsConfigured ? argumentsText : "Specify arguments"}
        onclick={() => setArgumentsOpen(!argumentsOpen)}
      >
        Arguments
        {#if argumentsConfigured}<span class="arguments-indicator" aria-hidden="true"></span>{/if}
      </button>
      {#if argumentsOpen}
        <div
          bind:this={argumentsPanel}
          class="arguments-popover"
          id="command-arguments-popover"
        >
          <input
            bind:this={argumentsInput}
            bind:value={argumentsText}
            id="command-arguments"
            class="input arguments-input"
            aria-label="Arguments"
            placeholder="--config=dev; program arguments after --"
            title="Shell-style quotes and escapes. Quote # to keep it literal. No variable or command expansion."
            autocomplete="off"
            spellcheck={false}
          />
        </div>
      {/if}
      <button
        type="button"
        class="button button-primary icon-button"
        aria-label={actionLabel}
        title={actionLabel}
        disabled={switchingWs ||
          (running ? !canStop(app.view) : !canSend(app.view, targetField))}
        onclick={submit}
      >
        {#if running}<Stop size={12} weight="fill" />{:else}<Play
            size={12}
            weight="fill"
          />{/if}
      </button>
    </div>
  </div>
  {#if app.view.error}<Notice tone="error">{app.view.error}</Notice>{/if}
  {#if st?.invocationNote}<Notice tone="error">{st.invocationNote}</Notice>{/if}
  {#if st?.besError}<Notice tone={st.besReady ? "warning" : "error"}
      >BES receiver: {st.besError}</Notice
    >{/if}
  {#if inputError}<p class="field-error composer-error" id="composer-target-error" role="status">{inputError}</p>{/if}

  {#if st === null}
    <div class="content dim">loading…</div>
  {:else if st.bazel === null}
    <div class="content">
      <Notice tone="error"
        >Bazel or Bazelisk was not found on your PATH. Install Bazel or
        Bazelisk and make sure it is available on your PATH. Then restart
        Zeal.</Notice
      >
    </div>
  {:else if st.workspace === null || switchingWs}
    <div class="content">
      <WorkspacePrompt
        initial={st.workspace ?? ""}
        initialScope={st.scope}
        onclose={st.workspace ? () => getSession().setWorkspacePromptOpen(false) : undefined}
      />
    </div>
  {:else}
    <div class="win-body">
      <aside class="sidebar">
        {@render targetPane(st)}
        <div
          class="divider"
          role="separator"
          aria-label="Resize target and invocation panes"
          aria-orientation="horizontal"
          onpointerdown={dividerDown}
        ></div>
        {@render invocationPane(st)}
      </aside>
      {@render workspaceContent(st, st.workspace)}
    </div>
  {/if}
</div>
