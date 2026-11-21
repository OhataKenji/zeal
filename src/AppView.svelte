<script lang="ts">
  import {
    ArrowClockwise,
    ChartBar,
    ClockCounterClockwise,
    Folders,
    HardDrives,
    MagnifyingGlass,
    Package,
    TreeStructure,
  } from "phosphor-svelte";
  import { canSend } from "./lib/session";
  import type { createSession, SessionView } from "./lib/session";
  import { invocationStatus } from "./lib/invocation";
  import { buildTree } from "./lib/model";
  import type { PaletteAction } from "./lib/palette";
  import { pages, type Page } from "./lib/pages";
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
  import ModulesView from "./lib/ModulesView.svelte";
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
        ? (snap?.targets.find((t) => t.label === selection.label)?.pkg ?? "")
        : "",
  );
  const invocation = $derived(view.invocation);

  let opening = $state(false);
  let switchError = $state<string | null>(null);
  let historyFilter = $state<"all" | "failed" | "running">("all");
  let treeRevealRequest = $state(0);

  // One surface owns the overlay layer at a time: the workspace prompt, the
  // command palette, or the run dialog. `opening` stays independent.
  type Surface =
    | { kind: "workspace"; path: string }
    | { kind: "palette" }
    | { kind: "run"; cmd: BazelCommand; pattern: string; flags: string };
  let surface = $state<Surface | null>(null);

  const canOpenPalette = $derived(
    st !== null && (surface === null || surface.kind === "palette"),
  );
  const promptPath = $derived(surface?.kind === "workspace" ? surface.path : null);
  const promptInitial = $derived(promptPath ?? st?.workspace ?? core?.lastWorkspace ?? "");

  const isMac =
    typeof navigator === "undefined"
      ? false
      : /macintosh|mac os|macintel/i.test(navigator.userAgent);

  const pageIcons = {
    workspace: HardDrives,
    targets: Folders,
    modules: Package,
    query: TreeStructure,
    history: ClockCounterClockwise,
    analytics: ChartBar,
  } satisfies Record<Page, typeof Folders>;

  const filteredInvocations = $derived(
    view.invocationRows.filter((row) => {
      if (historyFilter === "all") return true;
      const status = invocationStatus(row.record, row.execution);
      if (historyFilter === "running") return status.kind === "running" || status.kind === "stopping";
      return status.kind === "failed";
    }),
  );

  function openRunDialog(cmd: BazelCommand, pattern: string, flags = "") {
    surface = { kind: "run", cmd, pattern, flags };
  }

  function openWorkspacePrompt(path = "") {
    if (opening) return;
    switchError = null;
    surface = { kind: "workspace", path };
  }

  // Registered workspaces switch in place; anything else (BES history paths)
  // goes through the prompt so a stray click cannot start `bazel query //...`.
  async function switchWorkspace(path: string) {
    const registered = core?.workspaces.find((w) => w.workspace === path);
    if (!registered) {
      openWorkspacePrompt(path);
      return;
    }
    if (opening) return;
    opening = true;
    switchError = null;
    try {
      const ok = await session.openWorkspace(registered.workspace, registered.scope);
      if (ok && surface?.kind === "workspace") surface = null;
    } catch (e) {
      switchError = String(e);
    } finally {
      opening = false;
    }
  }

  // A switch error belongs to the workspace it failed on: the next attempt or
  // an actual workspace change clears it — unrelated state publishes do not.
  const workspacePath = $derived(st?.workspace ?? null);
  $effect(() => {
    void workspacePath;
    switchError = null;
  });

  // ⌘K/Ctrl+K toggles only while the palette or nothing owns the keyboard.
  function paletteShortcutAllowed(event: KeyboardEvent): boolean {
    return !event.shiftKey && !event.altKey && !event.repeat && canOpenPalette;
  }

  // A repeated ⌘O keeps the path already typed; explicit workspace picks
  // may still replace the prompt. With no workspace the prompt is already
  // the whole view, so the shortcut must not remount it.
  function openWorkspaceShortcut() {
    if (st && surface?.kind !== "workspace") openWorkspacePrompt("");
  }

  // An open run form keeps its edits; the prompt owns the keyboard too.
  function openRunShortcut() {
    if (canOpenPalette) openRunDialog("build", selectedTarget ?? "//...");
  }

  function onWindowKeydown(event: KeyboardEvent) {
    if (!(event.metaKey || event.ctrlKey) || event.isComposing) return;
    const key = event.key.toLowerCase();
    if (key === "o") {
      event.preventDefault();
      openWorkspaceShortcut();
    } else if (event.shiftKey && key === "r" && st) {
      event.preventDefault();
      openRunShortcut();
    } else if (key === "k" && paletteShortcutAllowed(event)) {
      event.preventDefault();
      surface = surface?.kind === "palette" ? null : { kind: "palette" };
    }
  }

  // Losing the workspace clears transient surfaces only; an explicit prompt
  // keeps its chosen path for the next workspace.
  $effect(() => {
    if (st === null && (surface?.kind === "run" || surface?.kind === "palette")) {
      surface = null;
    }
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
    surface = null;
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
      onswitch={switchWorkspace}
      onopennew={() => openWorkspacePrompt()}
    />
    {#if st !== null}
      <button
        type="button"
        class="toolbar-search"
        aria-label="Search targets, invocations, commands"
        disabled={!canOpenPalette}
        onclick={() => (surface = { kind: "palette" })}
      >
        <MagnifyingGlass size={13} />
        <span class="toolbar-search-label">Search targets, invocations, commands…</span>
        <span class="toolbar-search-kbd">{isMac ? "⌘K" : "Ctrl+K"}</span>
      </button>
    {/if}
  </header>

  {#if view.error}<Notice tone="error">{view.error}</Notice>{/if}
  {#if switchError}<Notice tone="error">{switchError}</Notice>{/if}
  {#if core?.besError}
    <Notice tone={core.besEndpoint ? "warning" : "error"}>BES receiver: {core.besError}</Notice>
  {/if}

  {#if core === null}
    <div class="content dim">loading…</div>
  {:else if st === null || surface?.kind === "workspace"}
    <div class="content">
      {#key promptPath ?? "initial"}
        <WorkspacePrompt
          initial={promptInitial}
          initialScope={{ include: ["//..."] }}
          bazelAvailable={core.bazel !== null}
          besEndpoint={core.besEndpoint}
          bind:opening
          onopen={session.openWorkspace}
          onclose={
            st
              ? () => {
                  if (surface?.kind === "workspace") surface = null;
                }
              : undefined
          }
        />
      {/key}
    </div>
  {:else}
    <div class="lens-workspace">
      <nav class="rail" aria-label="Workspace domains">
        {#each pages as { id, label } (id)}
          {@const Icon = pageIcons[id]}
          <button
            type="button"
            class="rail-btn"
            class:active={view.page === id}
            aria-current={view.page === id ? "page" : undefined}
            onclick={() => selectPage(id)}
          >
            <Icon size={16} />
            <span class="rail-lbl">{label}</span>
          </button>
        {/each}
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
              <Tree
                root={tree}
                activePkg={selectedPkg}
                revealRequest={treeRevealRequest}
                onselect={session.select}
              />
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

      {#snippet pageContent()}
        {#if view.page === "workspace"}
          <div class="page-center">
            <WorkspaceView
              workspace={st.workspace}
              scope={st.scope}
              info={view.workspaceInfo}
              loading={view.workspaceInfoLoading}
              error={view.workspaceInfoError}
              besEndpoint={core.besEndpoint}
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
          {#if snap}
            <PackageView
              index={snap}
              pkg={selectedPkg}
              selectedLabel={selectedTarget}
              canRun={canSend(view, "//...")}
              onselect={(label) => session.select({ kind: "target", label })}
              onrun={(cmd, pattern) => session.start(cmd, pattern)}
              onopenflags={openRunDialog}
            />
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
                onselecttarget={(label) => session.select({ kind: "target", label })}
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
      {/snippet}

      {#if view.page === "modules"}
        {#key st.workspace}
          <ModulesView workspace={st.workspace} bazelAvailable={core.bazel !== null} />
        {/key}
      {:else}
        <main class="content" class:invocation-content={view.page === "history"}>
          {@render pageContent()}
        </main>
      {/if}

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
      open={surface?.kind === "palette"}
      snapshot={snap}
      invocations={view.invocationRows}
      canRun={canSend(view, selectedTarget ?? "//...")}
      canRefresh={Boolean(st && core?.bazel && !view.snapshotLoading)}
      {isMac}
      onselect={onPaletteSelect}
      onclose={() => {
        if (surface?.kind === "palette") surface = null;
      }}
    />
    {#if surface?.kind === "run"}
      <RunDialog
        cmd={surface.cmd}
        pattern={surface.pattern}
        flags={surface.flags}
        canRun={(pattern) => canSend(view, pattern)}
        onrun={(cmd, pattern, flags) => session.start(cmd, pattern, flags)}
        onclose={() => {
          if (surface?.kind === "run") surface = null;
        }}
      />
    {/if}
  {/if}
</div>
