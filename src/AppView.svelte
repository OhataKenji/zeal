<script lang="ts">
  import {
    ArrowClockwise,
    ChartBar,
    ClockCounterClockwise,
    Folders,
    HardDrives,
    MagnifyingGlass,
    TreeStructure,
  } from "phosphor-svelte";
  import { canSend } from "./lib/session";
  import type { createSession, Page, SessionView } from "./lib/session";
  import { invocationStatus } from "./lib/invocation";
  import { buildTree } from "./lib/model";
  import type { PaletteAction } from "./lib/palette";
  import Tree from "./lib/Tree.svelte";
  import PackageView from "./lib/PackageView.svelte";
  import TargetView from "./lib/TargetView.svelte";
  import WorkspaceView from "./lib/WorkspaceView.svelte";
  import WorkspacePrompt from "./lib/WorkspacePrompt.svelte";
  import WorkspaceSwitcher from "./lib/WorkspaceSwitcher.svelte";
  import InvocationStatus from "./lib/InvocationStatus.svelte";
  import Notice from "./lib/Notice.svelte";
  import InvocationView from "./lib/InvocationView.svelte";
  import AnalyticsView from "./lib/AnalyticsView.svelte";
  import QueryView from "./lib/QueryView.svelte";
  import CommandPalette from "./lib/CommandPalette.svelte";
  import RunDialog from "./lib/RunDialog.svelte";
  import type { BazelCommand, InvocationRecord, OwnedInvocation } from "./lib/types";

  let consoleComponent: Promise<typeof import("./lib/ConsoleView.svelte")> | undefined;
  function loadConsole() {
    return (consoleComponent ??= import("./lib/ConsoleView.svelte"));
  }

  let { view, session }: { view: SessionView; session: ReturnType<typeof createSession> } =
    $props();

  const st = $derived(view.state);
  const core = $derived(view.core);
  const snap = $derived(view.snapshot);
  const tree = $derived(snap ? buildTree(snap) : null);
  const selection = $derived(view.selection);
  const selectedTarget = $derived(selection?.kind === "target" ? selection.label : null);
  const selectedPkg = $derived(
    selection?.kind === "package"
      ? selection.path
      : selection?.kind === "target"
        ? (snap?.targets.find((t) => t.label === selection.label)?.pkg ?? null)
        : null,
  );
  const invocation = $derived(view.invocation);

  let opening = $state(false);
  let openPromptPath = $state<string | null>(null);
  let historyFilter = $state<"all" | "failed" | "running">("all");
  let paletteOpen = $state(false);
  let treeRevealRequest = $state(0);

  const isMac =
    typeof navigator === "undefined"
      ? false
      : /macintosh|mac os|macintel/i.test(navigator.userAgent);

  let runDialogOpen = $state(false);
  let runDialogCmd = $state<BazelCommand>("build");
  let runDialogPattern = $state("//...");
  let runDialogFlags = $state("");

  const filteredInvocations = $derived(
    view.invocationRows.filter((row) => {
      if (historyFilter === "all") return true;
      const status = invocationStatus(row.record, row.execution);
      if (historyFilter === "running") return status.kind === "running" || status.kind === "stopping";
      return status.kind === "failed";
    }),
  );

  function openRunDialog(cmd: BazelCommand, pattern: string, flags = "") {
    paletteOpen = false;
    runDialogCmd = cmd;
    runDialogPattern = pattern;
    runDialogFlags = flags;
    runDialogOpen = true;
  }

  function openWorkspacePrompt(path = "") {
    paletteOpen = false;
    if (opening) return;
    openPromptPath = path;
  }

  // ⌘K/Ctrl+K toggles only while no prompt or run dialog owns the keyboard.
  function paletteShortcutAllowed(event: KeyboardEvent): boolean {
    return (
      !event.shiftKey &&
      !event.altKey &&
      !event.repeat &&
      st !== null &&
      openPromptPath === null &&
      !runDialogOpen
    );
  }

  function onWindowKeydown(event: KeyboardEvent) {
    if (!(event.metaKey || event.ctrlKey) || event.isComposing) return;
    const key = event.key.toLowerCase();
    if (key === "o") {
      event.preventDefault();
      openWorkspacePrompt("");
    } else if (event.shiftKey && key === "r" && st) {
      event.preventDefault();
      openRunDialog("build", selectedTarget ?? "//...");
    } else if (key === "k" && paletteShortcutAllowed(event)) {
      event.preventDefault();
      paletteOpen = !paletteOpen;
    }
  }

  // The palette only exists inside the workspace branch; losing the workspace
  // must not leave it armed for the next one.
  $effect(() => {
    if (st === null) paletteOpen = false;
  });

  function onPaletteSelect(action: PaletteAction) {
    switch (action.kind) {
      case "page":
        selectPage(action.page);
        break;
      case "run":
        openRunDialog("build", selectedTarget ?? "//...");
        break;
      case "refresh":
        session.refresh();
        break;
      case "open-workspace":
        openWorkspacePrompt("");
        break;
      case "target":
        session.select({ kind: "target", label: action.label });
        treeRevealRequest += 1;
        break;
      case "invocation":
        if (!filteredInvocations.some((row) => row.record.id === action.id)) {
          historyFilter = "all";
        }
        session.selectInvocation(action.id);
        break;
    }
  }

  function selectPage(page: Page) {
    openPromptPath = null;
    session.showPage(page);
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#snippet invocationRow(record: InvocationRecord, execution?: OwnedInvocation)}
  {@const startedAt = new Date(record.startedAt ?? NaN)}
  {@const status = invocationStatus(record, execution)}
  {@const active = view.page === "history" && view.selectedInvocationId === record.id}
  {@const owner = execution?.workspace}
  {@const workspace = owner ?? record.workspace}
  <li>
    <button
      class="invocation-row"
      title={`${record.cmd ?? "Command not recorded"} ${record.pattern ?? record.id} — ${status.label}${workspace ? ` — ${workspace}` : ""}`}
      aria-label={`${record.cmd ?? "Command not recorded"} ${record.pattern ?? record.id}: ${status.label}${owner ? ` — ${owner}` : ""}`}
      class:selected={active}
      aria-current={active ? "true" : undefined}
      onclick={() => session.selectInvocation(record.id)}
    >
      <span class="invocation-label">
        <InvocationStatus {...status} compact />
        <span class="invocation-command">{record.cmd ?? "?"}</span>
        {#if Number.isFinite(startedAt.getTime())}
          <time class="invocation-time" datetime={startedAt.toISOString()}>
            {startedAt.toLocaleString([], {
              month: "2-digit",
              day: "2-digit",
              hour: "2-digit",
              minute: "2-digit",
            })}
          </time>
        {:else}
          <span class="invocation-time">Time not recorded</span>
        {/if}
      </span>
      <span class="invocation-pattern">{record.pattern ?? record.id}</span>
      {#if owner && owner !== st?.workspace}
        <small class="invocation-owner">{owner}</small>
      {/if}
    </button>
  </li>
{/snippet}

<div class="win">
  <header class="toolbar">
    <WorkspaceSwitcher
      workspace={st?.workspace ?? null}
      {opening}
      onopen={(path) => openWorkspacePrompt(path ?? "")}
      onhome={() => selectPage("targets")}
    />
    {#if st !== null}
      <button
        type="button"
        class="toolbar-search"
        aria-label="Search targets, invocations, commands"
        disabled={openPromptPath !== null || runDialogOpen}
        onclick={() => (paletteOpen = true)}
      >
        <MagnifyingGlass size={13} />
        <span class="toolbar-search-label">Search targets, invocations, commands…</span>
        <span class="toolbar-search-kbd">{isMac ? "⌘K" : "Ctrl+K"}</span>
      </button>
    {/if}
  </header>

  {#if view.error}<Notice tone="error">{view.error}</Notice>{/if}
  {#if core?.besError}
    <Notice tone={core.besReady ? "warning" : "error"}>BES receiver: {core.besError}</Notice>
  {/if}

  {#if core === null}
    <div class="content dim">loading…</div>
  {:else if st === null || openPromptPath !== null}
    <div class="content">
      {#key openPromptPath ?? "initial"}
        <WorkspacePrompt
          initial={openPromptPath ?? st?.workspace ?? core.lastWorkspace ?? ""}
          initialScope={{ include: ["//..."] }}
          bazelAvailable={core.bazel !== null}
          bind:opening
          onopen={session.openWorkspace}
          onclose={st ? () => (openPromptPath = null) : undefined}
        />
      {/key}
    </div>
  {:else}
    <div class="lens-workspace">
      <nav class="rail" aria-label="Workspace domains">
        <button
          type="button"
          class="rail-btn"
          class:active={view.page === "workspace"}
          aria-current={view.page === "workspace" ? "page" : undefined}
          onclick={() => selectPage("workspace")}
        >
          <HardDrives size={16} />
          <span class="rail-lbl">Workspace</span>
        </button>
        <button
          type="button"
          class="rail-btn"
          class:active={view.page === "targets"}
          aria-current={view.page === "targets" ? "page" : undefined}
          onclick={() => selectPage("targets")}
        >
          <Folders size={16} />
          <span class="rail-lbl">Targets</span>
        </button>
        <button
          type="button"
          class="rail-btn"
          class:active={view.page === "query"}
          aria-current={view.page === "query" ? "page" : undefined}
          onclick={() => selectPage("query")}
        >
          <TreeStructure size={16} />
          <span class="rail-lbl">Query</span>
        </button>
        <button
          type="button"
          class="rail-btn"
          class:active={view.page === "history"}
          aria-current={view.page === "history" ? "page" : undefined}
          onclick={() => selectPage("history")}
        >
          <ClockCounterClockwise size={16} />
          <span class="rail-lbl">History</span>
        </button>
        <button
          type="button"
          class="rail-btn"
          class:active={view.page === "analytics"}
          aria-current={view.page === "analytics" ? "page" : undefined}
          onclick={() => selectPage("analytics")}
        >
          <ChartBar size={16} />
          <span class="rail-lbl">Analytics</span>
        </button>
      </nav>

      {#if view.page === "targets"}
        <aside class="sidebar">
          <div class="sidebar-head">
            <span>Packages</span>
            <button
              type="button"
              class="button button-ghost icon-button"
              aria-label="Reload targets"
              title="Reload targets"
              onclick={session.refresh}
              disabled={view.snapshotLoading}
            >
              <ArrowClockwise size={14} />
            </button>
          </div>
          <div class="sidebar-body">
            {#if view.snapshotError}
              <Notice tone={snap ? "warning" : "error"} onaction={session.refresh}>
                {view.snapshotError}
              </Notice>
            {/if}
            {#if snap && tree}
              <Tree root={tree} {selection} revealRequest={treeRevealRequest} onselect={session.select} />
            {:else if view.snapshotLoading}
              <p class="dim">Loading targets…</p>
            {:else}
              <p class="dim">—</p>
            {/if}
          </div>
        </aside>
      {:else if view.page === "history"}
        <aside class="sidebar">
          <div class="sidebar-head">
            <span>Invocations</span>
          </div>
          <div class="sidebar-filter">
            <span class="dim">Filter</span>
            <div class="sub-switch">
              <button
                type="button"
                class:active={historyFilter === "all"}
                onclick={() => (historyFilter = "all")}>All</button
              >
              <button
                type="button"
                class:active={historyFilter === "failed"}
                onclick={() => (historyFilter = "failed")}>Failed</button
              >
              <button
                type="button"
                class:active={historyFilter === "running"}
                onclick={() => (historyFilter = "running")}>Running</button
              >
            </div>
          </div>
          <div class="sidebar-body">
            <ul class="list">
              {#each filteredInvocations as row (row.record.id)}
                {@render invocationRow(row.record, row.execution)}
              {:else}
                <li class="dim empty-row">No invocations.</li>
              {/each}
            </ul>
            {#if view.invocationsError}
              <Notice tone="error" onaction={session.loadInvocations}>
                {view.invocationsError}
              </Notice>
            {/if}
          </div>
        </aside>
      {/if}

      <main class="content" class:invocation-content={view.page === "history"}>
        {#if view.page === "workspace"}
          <div class="page-center">
            <WorkspaceView
              workspace={st.workspace}
              scope={st.scope}
              info={view.workspaceInfo}
              loading={view.workspaceInfoLoading}
              error={view.workspaceInfoError}
              onretry={session.loadWorkspaceInfo}
              onsavescope={(scope) => session.openWorkspace(st.workspace, scope)}
            />
          </div>
        {:else if view.page === "targets"}
          {#if snap?.meta.status === "partial"}
            <Notice tone="warning">
              Partial target list{view.snapshotError ? ` — ${view.snapshotError}` : ""}
            </Notice>
          {:else if snap && view.snapshotError}
            <Notice tone="warning" onaction={session.refresh}>
              Refresh failed — showing the snapshot from {new Date(
                snap.meta.fetchedAt,
              ).toLocaleTimeString()} ({view.snapshotError})
            </Notice>
          {/if}
          {#if snap && selectedPkg !== null}
            <PackageView
              index={snap}
              pkg={selectedPkg}
              selectedLabel={selectedTarget}
              canRun={canSend(view, "//...")}
              onselect={(label) => session.select({ kind: "target", label })}
              onrun={(cmd, pattern) => session.start(cmd, pattern)}
              onopenflags={openRunDialog}
            />
          {:else if snap}
            <p class="workspace-empty dim">Select a package or target in the sidebar.</p>
          {:else if view.snapshotLoading}
            <p class="dim">Loading targets…</p>
          {:else if view.snapshotError}
            <Notice tone="error" onaction={session.refresh}>{view.snapshotError}</Notice>
          {/if}
        {:else if view.page === "history"}
          {#if invocation?.execution?.console}
            {#key `${invocation.execution.workspace}:${invocation.execution.id}`}
              <div class="console-container">
                {#await loadConsole() then { default: ConsoleView }}
                  <ConsoleView
                    record={invocation.record}
                    execution={invocation.execution}
                    onstop={session.stop}
                  />
                {/await}
              </div>
            {/key}
          {:else if view.selectedInvocationId}
            {#key view.selectedInvocationId}
              <InvocationView
                {view}
                reload={session.loadDetail}
                reloadProfile={session.loadProfile}
                onstop={session.stop}
                onrerun={(cmd, pattern) => session.start(cmd, pattern)}
              />
            {/key}
          {:else}
            <p class="workspace-empty dim">Select an invocation from the sidebar.</p>
          {/if}
        {:else if view.page === "query"}
          <div class="page-center">
            {#key st.workspace}
              <QueryView
                {snap}
                workspace={st.workspace}
                bazelAvailable={core !== null && core.bazel !== null}
              />
            {/key}
          </div>
        {:else if view.page === "analytics"}
          <div class="page-center">
            {#key st.workspace}
              <AnalyticsView workspace={st.workspace} invocations={view.invocations} />
            {/key}
          </div>
        {/if}
      </main>

      {#if view.page === "targets" && snap && selectedTarget}
        <aside class="inspector" aria-label="Target details">
          {#if view.targetDetail?.target.label === selectedTarget}
            {#if view.targetDetail.status === "partial"}
              <Notice tone="warning">Some target details could not be loaded.</Notice>
            {/if}
            {#key `${selectedTarget}:${snap.meta.fetchedAt}`}
              <TargetView
                index={snap}
                detail={view.targetDetail}
                outputs={view.targetOutputs}
                outputsError={view.targetOutputsError}
                actions={view.targetActions}
                actionsError={view.targetActionsError}
                wsRoot={st.workspace}
                canRun={canSend(view, selectedTarget)}
                onselect={(label) => session.select({ kind: "target", label })}
                onrun={(cmd, pattern) => session.start(cmd, pattern)}
                onopenflags={openRunDialog}
                onclose={() =>
                  session.select(selectedPkg ? { kind: "package", path: selectedPkg } : null)}
              />
            {/key}
          {:else if view.targetDetailError}
            <Notice
              tone="error"
              onaction={() =>
                view.snapshotError ? session.refresh() : session.select(selection)}
            >
              Could not load target details: {view.targetDetailError}
            </Notice>
          {:else}
            <p class="dim">Loading target details…</p>
          {/if}
        </aside>
      {/if}
    </div>

    <CommandPalette
      bind:open={paletteOpen}
      snapshot={snap}
      invocations={view.invocationRows}
      canRun={canSend(view, selectedTarget ?? "//...")}
      canRefresh={Boolean(st && core?.bazel && !view.snapshotLoading)}
      {isMac}
      onselect={onPaletteSelect}
    />
    <RunDialog
      bind:open={runDialogOpen}
      bind:cmd={runDialogCmd}
      bind:pattern={runDialogPattern}
      bind:flags={runDialogFlags}
      canRun={(pattern) => canSend(view, pattern)}
      onrun={(cmd, pattern, flags) => session.start(cmd, pattern, flags)}
    />
  {/if}
</div>
