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
  import {
    localDay,
    matchesHistoryFilter,
    rowStatus,
    type HistoryFilter,
    type InvocationRow,
  } from "./lib/invocation";
  import { buildTree, packageContents } from "./lib/model";
  import type { PaletteAction } from "./lib/palette";
  import { pages, type Page } from "./lib/pages";
  import { createIndex } from "./lib/search";
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
  import type { BazelCommand } from "./lib/types";

  let consoleComponent: Promise<typeof import("./lib/ConsoleView.svelte")> | undefined;
  function loadConsole() {
    return (consoleComponent ??= import("./lib/ConsoleView.svelte"));
  }

  let { view, session }: { view: SessionView; session: ReturnType<typeof createSession> } =
    $props();

  const st = $derived(view.state);
  const core = $derived(view.core);
  const snap = $derived(view.snapshot);
  const tree = $derived(snap ? buildTree(snap.targets) : null);
  const targetIndex = $derived(snap ? createIndex(snap.targets) : null);
  const selection = $derived(view.selection);
  const selectedTarget = $derived(selection?.kind === "target" ? selection.label : null);
  const pkg = $derived.by(() => {
    if (view.page !== "targets" || selectedTarget !== null || !tree) return null;
    const node = selection?.kind === "package" ? tree.byPath.get(selection.path) : tree.root;
    return node ? packageContents(node) : null;
  });
  const invocation = $derived(view.invocation);

  let opening = $state(false);
  let switchError = $state<string | null>(null);
  let historyFilter = $state<HistoryFilter>("all");
  let historyDay = $state<string | null>(null);

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

  function rowDuration(row: InvocationRow): number {
    const { startedAt, finishedAt } = row.record;
    return startedAt !== undefined && finishedAt !== undefined
      ? Math.max(0, finishedAt - startedAt)
      : -1;
  }

  const filteredInvocations = $derived.by(() => {
    const rows = view.invocationRows.filter(
      (row) =>
        matchesHistoryFilter(row, historyFilter) &&
        (historyDay === null || localDay(row.record.startedAt) === historyDay),
    );
    return historyDay === null ? rows : rows.toSorted((a, b) => rowDuration(b) - rowDuration(a));
  });

  function openRunDialog(cmd: BazelCommand, pattern: string, flags = "") {
    surface = { kind: "run", cmd, pattern, flags };
  }

  function openWorkspacePrompt(path = "") {
    if (opening) return;
    switchError = null;
    surface = { kind: "workspace", path };
  }

  function closeWorkspacePrompt() {
    if (surface?.kind === "workspace") surface = null;
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
    historyDay = null;
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
    if (!canOpenPalette) return;
    if (pkg) {
      if (pkg.targets.length && canSend(view, pkg.pattern)) {
        openRunDialog(pkg.primaryCmd, pkg.pattern);
      }
    } else {
      openRunDialog("build", selectedTarget ?? "//...");
    }
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
        openRunShortcut();
        break;
      case "refresh":
        session.refresh();
        break;
      case "open-workspace":
        openWorkspacePrompt("");
        break;
      case "target":
        selectTarget(action.label);
        break;
      case "invocation":
        selectInvocation(action.id);
        break;
    }
  }

  function selectInvocation(id: string) {
    if (!filteredInvocations.some((row) => row.record.id === id)) {
      historyFilter = "all";
      historyDay = null;
    }
    session.selectInvocation(id);
  }

  function openHistoryDay(day: string) {
    historyDay = day;
    historyFilter = "all";
    session.showPage("history");
  }

  function selectPage(page: Page) {
    surface = null;
    session.showPage(page);
  }

  function selectTarget(label: string) {
    session.select({ kind: "target", label });
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#snippet invocationRow(row: InvocationRow)}
  {@const { record, execution } = row}
  {@const startedAt = new Date(record.startedAt ?? NaN)}
  {@const status = rowStatus(row)}
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
      onreturn={st && surface?.kind === "workspace" ? closeWorkspacePrompt : undefined}
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
          onclose={st ? closeWorkspacePrompt : undefined}
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
            <button
              type="button"
              class="sidebar-root"
              title="Show root package"
              onclick={() => session.select(null)}>Targets</button
            >
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
              <Tree root={tree.root} {selection} onselect={session.select} />
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
            {#if historyDay}
              <button
                type="button"
                class="button button-ghost"
                title="Clear day filter"
                aria-label={`Clear day filter ${historyDay}`}
                onclick={() => (historyDay = null)}>{historyDay} ×</button
              >
            {/if}
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
                {@render invocationRow(row)}
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
            {#key st.workspace}
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
            {/key}
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
            <div class="page-center">
              {#if selectedTarget !== null}
                {#if view.targetDetail?.target.label === selectedTarget}
                  {#if view.targetDetail.status === "partial"}
                    <Notice tone="warning">Some target details could not be loaded.</Notice>
                  {/if}
                  {#key `${selectedTarget}:${snap.meta.fetchedAt}`}
                    <TargetView
                      index={snap}
                      detail={view.targetDetail}
                      phaseData={view.targetPhase}
                      phaseError={view.targetPhaseError}
                      tab={view.targetTab}
                      flags={view.targetFlags}
                      wsRoot={st.workspace}
                      canRun={canSend(view, selectedTarget)}
                      onselect={selectTarget}
                      onrun={session.start}
                      onopenflags={openRunDialog}
                      ontab={session.setTargetTab}
                      onapply={session.applyTargetFlags}
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
              {:else if pkg}
                {#key pkg.pattern}
                  <PackageView
                    {pkg}
                    canRun={canSend(view, pkg.pattern)}
                    onselect={selectTarget}
                    onrun={session.start}
                    onopenflags={openRunDialog}
                  />
                {/key}
              {/if}
            </div>
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
                onstop={session.stop}
                onrerun={(cmd, pattern) => session.start(cmd, pattern)}
                onselecttarget={selectTarget}
                ontab={session.setInvocationTab}
              />
            {/key}
          {:else}
            <p class="workspace-empty dim">Select an invocation from the sidebar.</p>
          {/if}
        {/if}
        <div class="page-center" hidden={view.page !== "query"}>
          {#key st.workspace}
            <QueryView
              {targetIndex}
              workspace={st.workspace}
              bazelAvailable={core !== null && core.bazel !== null}
              onselecttarget={selectTarget}
            />
          {/key}
        </div>
        <div class="page-center" hidden={view.page !== "analytics"}>
          {#key st.workspace}
            <AnalyticsView
              workspace={st.workspace}
              invocations={view.invocations}
              onselectday={openHistoryDay}
            />
          {/key}
        </div>
      {/snippet}

      {#if view.page === "modules" && view.modules}
        {#key st.workspace}
          <ModulesView
            view={view.modules}
            bazelAvailable={core.bazel !== null}
            onload={session.loadModules}
            onselect={session.selectModule}
            onexplain={session.explainModule}
          />
        {/key}
      {/if}
      <main
        class="content"
        class:invocation-content={view.page === "history"}
        hidden={view.page === "modules" && Boolean(view.modules)}
      >
        {@render pageContent()}
      </main>

    </div>

    <CommandPalette
      open={surface?.kind === "palette"}
      {targetIndex}
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
        {targetIndex}
        canRun={(pattern) => canSend(view, pattern)}
        onrun={(cmd, pattern, flags) => session.start(cmd, pattern, flags)}
        onclose={() => {
          if (surface?.kind === "run") surface = null;
        }}
      />
    {/if}
  {/if}
</div>
