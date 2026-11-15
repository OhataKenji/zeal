<script lang="ts">
  import {
    CaretDown,
    CaretRight,
    ChartBar,
    ArrowClockwise,
    ClockCounterClockwise,
    Folders,
    Play,
    SlidersHorizontal,
    TreeStructure,
  } from "phosphor-svelte";
  import { onMount, tick } from "svelte";
  import { app, connect, getSession } from "./lib/state.svelte";
  import { canSend } from "./lib/session";
  import { invocationStatus } from "./lib/invocation";
  import { createIndex } from "./lib/search";
  import { targetError } from "./lib/target";
  import { buildTree } from "./lib/model";
  import Tree from "./lib/Tree.svelte";
  import PackageView from "./lib/PackageView.svelte";
  import TargetView from "./lib/TargetView.svelte";
  import WorkspacePrompt from "./lib/WorkspacePrompt.svelte";
  import WorkspaceSwitcher from "./lib/WorkspaceSwitcher.svelte";
  import InvocationStatus from "./lib/InvocationStatus.svelte";
  import Notice from "./lib/Notice.svelte";
  import InvocationView from "./lib/InvocationView.svelte";
  import AnalyticsView from "./lib/AnalyticsView.svelte";
  import QueryView from "./lib/QueryView.svelte";
  import SearchInput from "./lib/SearchInput.svelte";
  import type {
    BazelCommand,
    ExplorerSelection,
    InvocationRecord,
    OwnedInvocation,
    TargetScope,
    WorkspaceState,
  } from "./lib/types";

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
  const core = $derived(app.view.core);
  const snap = $derived(app.view.snapshot);
  const tree = $derived(snap ? buildTree(snap) : null);
  const selection = $derived(app.view.selection);
  const selected = $derived(selection?.kind === "target" ? selection.label : null);
  const content = $derived(app.view.content);
  const consoleView = $derived(
    app.view.console && app.view.execution
      ? { record: app.view.console, execution: app.view.execution }
      : null,
  );

  let revealRequest = $state(0);
  let targetField = $state("");
  let cmd = $state<BazelCommand>("build");
  let argumentsText = $state("");
  let argumentsOpen = $state(false);
  let argumentsToggle: HTMLButtonElement | undefined;
  let argumentsInput = $state<HTMLInputElement>();
  const argumentsConfigured = $derived(argumentsText.trim() !== "");
  const switchingWs = $derived(app.view.workspacePromptOpen);
  let workspaceForm = $state<{
    mode: "open" | "settings";
    path: string;
    scope: TargetScope;
  } | null>(null);

  function refresh() {
    getSession().refresh();
  }

  function editWorkspace(mode: "open" | "settings", path = "") {
    workspaceForm = {
      mode,
      path: mode === "settings" ? st?.workspace ?? "" : path,
      scope:
        mode === "settings"
          ? st?.scope ?? { include: ["//..."] }
          : { include: ["//..."] },
    };
    getSession().setWorkspacePromptOpen(true);
  }

  const settingsOpen = $derived(switchingWs && workspaceForm?.mode === "settings");

  function workspaceHome() {
    if (!st?.workspace || app.view.opening) return;
    getSession().setWorkspacePromptOpen(false);
    getSession().showPage("targets");
  }

  function toggleWorkspaceSettings() {
    if (app.view.opening) return;
    if (settingsOpen) getSession().setWorkspacePromptOpen(false);
    else editWorkspace("settings");
  }

  let composerWorkspace: string | null = null;
  $effect(() => {
    const path = st?.workspace ?? null;
    if (path !== composerWorkspace) {
      composerWorkspace = path;
      targetField = "";
      argumentsText = "";
      argumentsOpen = false;
      cmd = "build";
    }
  });
  const actionLabel = $derived(cmd === "test" ? "Test" : cmd === "run" ? "Run" : "Build");
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
    getSession().start(cmd, targetField, argumentsText);
  }

  async function setArgumentsOpen(open: boolean) {
    argumentsOpen = open || argumentsConfigured;
    await tick();
    (open ? argumentsInput : argumentsToggle)?.focus();
  }

  function argumentsKey(event: KeyboardEvent) {
    if (event.isComposing || event.key !== "Escape") return;
    event.preventDefault();
    void setArgumentsOpen(false);
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

{#snippet targetPane()}
  <section
    class="pane"
    class:collapsed={collapsed.targets}
    style:flex-grow={collapsed.targets ? 0 : targetsShare}
  >
    <div class="pane-toolbar">
      <button
        class="pane-head"
        aria-expanded={!collapsed.targets}
        onclick={() => (collapsed.targets = !collapsed.targets)}
      >
        {#if collapsed.targets}<CaretRight size={10} />{:else}<CaretDown size={10} />{/if}
        <Folders size={18} />
        Targets
      </button>
      <button
        type="button"
        class="button button-ghost icon-button"
        aria-label="Reload targets"
        title="Reload targets"
        onclick={refresh}
        disabled={app.view.snapshotLoading}
      >
        <ArrowClockwise size={14} />
      </button>
    </div>
    {#if !collapsed.targets}
      <div class="pane-body">
        {#if app.view.snapshotError}
          <Notice tone={snap ? "warning" : "error"} onaction={refresh}>
            {app.view.snapshotError}
          </Notice>
        {/if}
        {#if snap && tree}
          <Tree root={tree} {selection} {revealRequest} onselect={select} />
        {:else if app.view.snapshotLoading}
          <p class="dim">Loading targets…</p>
        {:else}
          <p class="dim">—</p>
        {/if}
      </div>
    {/if}
  </section>
{/snippet}

{#snippet invocationRow(record: InvocationRecord, execution?: OwnedInvocation)}
  {@const startedAt = new Date(record.startedAt ?? NaN)}
  {@const status = invocationStatus(record, execution)}
  {@const active = content.kind === "invocation" && content.id === record.id}
  <li>
    <button
      class="invocation-row"
      title={`${record.cmd ?? "Command not recorded"} ${record.pattern ?? record.id} — ${status.label}${record.workspace ? ` — ${record.workspace}` : ""}`}
      aria-label={`${record.cmd ?? "Command not recorded"} ${record.pattern ?? record.id}: ${status.label}`}
      class:selected={active}
      aria-current={active ? "true" : undefined}
      onclick={() => getSession().selectInvocation(record.id)}
    >
      <span class="invocation-label"
        ><InvocationStatus {...status} compact /><span class="invocation-command"
          >{record.cmd ?? "?"}</span
        >
        {#if Number.isFinite(startedAt.getTime())}
        <time class="invocation-time" datetime={startedAt.toISOString()}
          >{startedAt.toLocaleString([], {
            year: "numeric",
            month: "2-digit",
            day: "2-digit",
            hour: "2-digit",
            minute: "2-digit",
          })}</time
        >{:else}<span class="invocation-time">Time not recorded</span>{/if}</span
      >
      <span class="invocation-pattern">{record.pattern ?? record.id}</span>
    </button>
  </li>
{/snippet}

{#snippet invocationPane(state: WorkspaceState)}
  <section
    class="pane"
    class:collapsed={collapsed.invocations}
    style:flex-grow={collapsed.invocations ? 0 : 1 - targetsShare}
  >
    <button
      class="pane-head"
      aria-expanded={!collapsed.invocations}
      onclick={() => (collapsed.invocations = !collapsed.invocations)}
    >
      {#if collapsed.invocations}<CaretRight size={10} />{:else}<CaretDown size={10} />{/if}
      <ClockCounterClockwise size={18} />
      Invocations
    </button>
    {#if !collapsed.invocations}
      <div class="pane-body">
        <ul class="list">
          {#each app.view.invocationRows as row (row.record.id)}
            {@render invocationRow(row.record, row.execution)}
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

{#snippet workspaceContent(workspace: string)}
  <main
    class="content"
    class:invocation-content={content.kind !== "targets"}
  >
    {#if consoleView}
      {#key `${consoleView.execution.workspace}:${consoleView.execution.id}`}
        <div class="console-container">
          {#await loadConsole() then { default: ConsoleView }}
          <ConsoleView record={consoleView.record}
            execution={consoleView.execution}
            onstop={() => getSession().stop(consoleView.execution.id)} />
          {/await}
        </div>
      {/key}
    {:else}
    {#if snap?.meta.status === "partial"}
      <Notice tone="warning">
        Partial target list{app.view.snapshotError ? ` — ${app.view.snapshotError}` : ""}
      </Notice>
    {:else if snap && app.view.snapshotError}
      <Notice tone="warning" onaction={refresh}>
        Refresh failed — showing the snapshot from {new Date(
          snap.meta.fetchedAt,
        ).toLocaleTimeString()} ({app.view.snapshotError})
      </Notice>
    {/if}
    {#if app.view.snapshotLoading}
      <p class="dim">Loading targets…</p>
    {/if}
    {#if content.kind === "invocation"}
      {#key content.id}
        <InvocationView
          view={app.view}
          reload={getSession().loadDetail}
          reloadProfile={getSession().loadProfile}
          onstop={getSession().stop}
        />
      {/key}
    {:else if content.kind === "analytics"}
      {#key workspace}<AnalyticsView {workspace} invocations={app.view.invocations} />{/key}
    {:else if content.kind === "query"}
      {#key workspace}
        <QueryView {snap} {workspace} />
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
          app.view.snapshotError ? refresh() : getSession().select(selection)}>
          Could not load target details: {app.view.targetDetailError}
        </Notice>
      {:else}
        <p class="dim">Loading target details…</p>
      {/if}
    {:else if snap}
      <p class="workspace-empty dim">Select a package or target in the sidebar, or search for a target above.</p>
    {:else if !app.view.snapshotLoading && app.view.snapshotError}
      <Notice tone="error" announce={collapsed.targets} onaction={refresh}>
        {app.view.snapshotError}
      </Notice>
    {/if}
    {/if}
  </main>
{/snippet}

<div class="win">
  <div class="toolbar">
    <WorkspaceSwitcher
      workspace={st?.workspace ?? null}
      {settingsOpen}
      opening={app.view.opening}
      onopen={(path) => editWorkspace("open", path)}
      onsettings={toggleWorkspaceSettings}
      onhome={workspaceHome}
    />
    <div class="composer" class:composer-invalid={!!inputError}>
      <select class="select" aria-label="Bazel command" bind:value={cmd}>
        <option>build</option>
        <option>test</option>
        <option>run</option>
      </select>
      <SearchInput
        class="input composer-target"
        aria-label="Target label or pattern"
        aria-invalid={!!inputError}
        aria-describedby={inputError ? "composer-target-error" : undefined}
        placeholder="Search targets or enter a pattern"
        {index}
        value={targetField}
        oninput={(v) => (targetField = v)}
        onpick={pickTarget}
      />
      <button
        bind:this={argumentsToggle}
        type="button"
        class="button button-ghost icon-button arguments-toggle"
        aria-label="Arguments"
        aria-expanded={argumentsOpen}
        aria-controls={argumentsOpen ? "command-arguments" : undefined}
        title="Arguments"
        onclick={() => setArgumentsOpen(!argumentsOpen)}
      >
        <SlidersHorizontal size={16} />
      </button>
      <button
        type="button"
        class="button button-primary icon-button"
        aria-label={actionLabel}
        title={actionLabel}
        disabled={switchingWs || !canSend(app.view, targetField)}
        onclick={submit}
      >
        <Play size={12} weight="fill" />
      </button>
      {#if argumentsOpen}
        <div class="arguments-row">
          <label for="command-arguments">Arguments</label>
          <input
            bind:this={argumentsInput}
            bind:value={argumentsText}
            id="command-arguments"
            class="input arguments-input"
            onkeydown={argumentsKey}
            placeholder={cmd === "run" ? "--config=dev -- --port=8080" : "--config=dev --keep_going"}
            title="Shell-style quotes and escapes. Quote # to keep it literal. No variable or command expansion."
            autocomplete="off"
            spellcheck={false}
          />
        </div>
      {/if}
    </div>
  </div>
  {#if app.view.error}<Notice tone="error">{app.view.error}</Notice>{/if}
  {#if core?.besError}<Notice tone={core.besReady ? "warning" : "error"}
      >BES receiver: {core.besError}</Notice
    >{/if}
  {#if inputError}<p class="field-error composer-error" id="composer-target-error" role="status">{inputError}</p>{/if}

  {#if core === null}
    <div class="content dim">loading…</div>
  {:else if st === null || switchingWs}
    <div class="content">
      {#key `${workspaceForm?.mode ?? "open"}:${workspaceForm?.path ?? ""}`}
        <WorkspacePrompt
          mode={workspaceForm?.mode ?? "open"}
          initial={workspaceForm?.path ?? st?.workspace ?? core.lastWorkspace ?? ""}
          initialScope={workspaceForm?.scope ?? st?.scope ?? { include: ["//..."] }}
          bazelAvailable={core.bazel !== null}
          onclose={st ? () => getSession().setWorkspacePromptOpen(false) : undefined}
        />
      {/key}
    </div>
  {:else}
    <div class="win-body">
      <aside class="sidebar">
        {@render targetPane()}
        <div
          class="divider"
          role="separator"
          aria-label="Resize target and invocation panes"
          aria-orientation="horizontal"
          onpointerdown={dividerDown}
        ></div>
        {@render invocationPane(st)}
        <nav class="workspace-nav" aria-label="Workspace views">
          <button type="button" class="workspace-nav-link" class:active={content.kind === "query"} aria-current={content.kind === "query" ? "page" : undefined} onclick={() => getSession().showPage("query")}><TreeStructure size={18} />Query</button>
          <button type="button" class="workspace-nav-link" class:active={content.kind === "analytics"} aria-current={content.kind === "analytics" ? "page" : undefined} onclick={() => getSession().showPage("analytics")}><ChartBar size={18} />Analytics</button>
        </nav>
      </aside>
      {@render workspaceContent(st.workspace)}
    </div>
  {/if}
</div>
