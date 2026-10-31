<script lang="ts">
  import {
    Cube,
    CaretDown,
    CaretRight,
    ChartBar,
    ArrowClockwise,
    Play,
    Stop,
  } from "phosphor-svelte";
  import { onMount, tick } from "svelte";
  import { app, connect, getSession, refresh } from "./lib/state.svelte";
  import { canSend, canStop } from "./lib/session";
  import { runStatus } from "./lib/run";
  import {
    createIndex,
    keyAction,
    LIST_ID,
    moveIndex,
    optionId,
    search,
  } from "./lib/search";
  import { targetError } from "./lib/target";
  import { buildTree } from "./lib/model";
  import Tree from "./lib/Tree.svelte";
  import TargetView from "./lib/TargetView.svelte";
  import WorkspacePrompt from "./lib/WorkspacePrompt.svelte";
  import RunStatus from "./lib/RunStatus.svelte";
  import RunView from "./lib/RunView.svelte";
  import AnalyticsView from "./lib/AnalyticsView.svelte";
  import SearchDropdown from "./lib/SearchDropdown.svelte";
  import type { AppState, RunCmd } from "./lib/types";

  let consoleComponent: Promise<typeof import("./lib/ConsoleView.svelte")> | undefined;
  function loadConsole() {
    return consoleComponent ??= import("./lib/ConsoleView.svelte");
  }

  onMount(connect);

  const st = $derived(app.state);
  const snap = $derived(st?.snapshot ?? null);
  const tree = $derived(snap ? buildTree(snap) : null);
  const known = $derived(new Set(snap?.targets.map((t) => t.label) ?? []));

  let analyticsOpen = $state(false);
  let selected = $state<string | null>(null);
  let revealRequest = $state(0);
  let targetField = $state("");
  let cmd = $state<RunCmd>("build");
  let argumentsText = $state("");
  let argumentsOpen = $state(false);
  let argumentsToggle: HTMLButtonElement | undefined;
  let argumentsInput = $state<HTMLInputElement>();
  const argumentsConfigured = $derived(argumentsText.trim() !== "");
  let switchingWs = $state(false);
  const busy = $derived(
    !app.view.ready ||
      app.view.starting ||
      app.view.opening ||
      !!st?.running ||
      st?.ingest === "running",
  );
  const running = $derived(!!st?.running);
  const actionLabel = $derived(
    running ? "Stop" : cmd === "test" ? "Test" : cmd === "run" ? "Run" : "Build",
  );
  const inputError = $derived(
    targetField === "" ? null : targetError(targetField),
  );

  // The composer is the only search entry point. `dropdownOpen` is set by
  // typing only: programmatic insertion (tree, relations, run rows) replaces
  // the field without dropping the list over it. The matches are computed even
  // while closed, because ↑/↓ reopen the list from the current text.
  let dropdownOpen = $state(false);
  let matchIndex = $state(0);
  const index = $derived(snap ? createIndex(snap.targets) : null);
  const matches = $derived(index ? search(index, targetField) : []);
  // A snapshot swap can shorten the list under the selection, so clamp here:
  // the highlight, `aria-activedescendant` and Enter all name the same row.
  const activeIndex = $derived(moveIndex(matchIndex, 0, matches.length));
  const activeOption = $derived(
    dropdownOpen && matches.length > 0 ? optionId(activeIndex) : undefined,
  );

  function pickTarget(value: string) {
    select(value);
    collapsed.targets = false;
    revealRequest += 1;
    dropdownOpen = false;
    matchIndex = 0;
  }

  function composerKey(event: KeyboardEvent) {
    if (event.isComposing) return;
    const action = keyAction(
      { open: dropdownOpen, index: activeIndex },
      event.key,
      matches,
    );
    switch (action.kind) {
      case "move":
        event.preventDefault();
        dropdownOpen = true;
        matchIndex = action.index;
        break;
      case "pick":
        event.preventDefault();
        pickTarget(action.value);
        break;
      case "close":
        dropdownOpen = false;
        break;
      case "none":
        break;
    }
  }

  function submit() {
    if (running) {
      void getSession().stop();
    } else {
      void getSession().start(cmd, targetField, argumentsText);
    }
  }

  async function setArgumentsOpen(open: boolean) {
    argumentsOpen = open;
    dropdownOpen = false;
    await tick();
    if (argumentsOpen === open) {
      (open ? argumentsInput : argumentsToggle)?.focus();
    }
  }

  function argumentsKey(event: KeyboardEvent) {
    if (event.isComposing || event.key !== "Escape") return;
    event.preventDefault();
    void setArgumentsOpen(false);
  }

  // Sidebar split: share of height given to the targets pane, and per-pane
  // collapse. The divider drags the boundary like VS Code's stacked views.
  let targetsShare = $state(0.55);
  let collapsed = $state({ targets: false, runs: false });

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

  // Every selection — tree, relation line — also lands in the composer target
  // field, which is the single source of truth for "the current target".
  function select(label: string | null) {
    analyticsOpen = false;
    getSession().selectRun(null);
    selected = label;
    if (label) targetField = label;
  }

  // Keep a valid selection across refreshes and workspace switches.
  $effect(() => {
    if (!snap) {
      selected = null;
      return;
    }
    if (!selected || !known.has(selected)) {
      selected = snap.targets[0]?.label ?? null;
      if (selected) targetField = selected;
    }
  });
</script>

{#snippet targetPane(state: AppState)}
  <section
    class="pane"
    class:collapsed={collapsed.targets}
    style:flex-grow={collapsed.targets ? 0 : targetsShare}
  >
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
    {#if !collapsed.targets}
      <div class="pane-body">
        {#if snap && tree}
          <Tree root={tree} {selected} {revealRequest} onselect={select} />
        {:else if state.ingest === "running"}
          <p class="dim">Loading targets…</p>
        {:else if state.lastError}
          <p class="error">{state.lastError.summary}</p>
          <button type="button" class="button" onclick={refresh}>retry</button>
        {:else}
          <p class="dim">—</p>
        {/if}
      </div>
    {/if}
  </section>
{/snippet}

{#snippet runPane(state: AppState)}
  <section
    class="pane"
    class:collapsed={collapsed.runs}
    style:flex-grow={collapsed.runs ? 0 : 1 - targetsShare}
  >
    <div class="pane-heading">
      <button
        class="pane-head"
        aria-expanded={!collapsed.runs}
        onclick={() => (collapsed.runs = !collapsed.runs)}
      >
        {#if collapsed.runs}<CaretRight size={10} />{:else}<CaretDown
            size={10}
          />{/if} Runs
      </button>
      {#if state.consoleRun ?? state.running}
        {@const consoleId = (state.consoleRun ?? state.running)!}
        <button
          class="analytics-link"
          class:active={app.view.consoleOpen}
          onclick={() => getSession().openConsole(
            app.view.console?.id === consoleId
              ? app.view.console
              : { id: consoleId },
          )}>Console</button>
      {/if}
      <button
        class="analytics-link"
        class:active={analyticsOpen && !app.view.selectedRun}
        onclick={() => {
          analyticsOpen = true;
          getSession().selectRun(null);
        }}><ChartBar size={12} /> Analytics</button
      >
    </div>
    {#if !collapsed.runs}
      <div class="pane-body">
        <ul class="list">
          {#each app.view.runs as run (run.id)}
            {@const status = runStatus(run, state.running, state.stopping)}
            {@const active =
              app.view.selectedRun === run.id ||
              (app.view.consoleOpen && app.view.console?.id === run.id)}
            <li>
              <button
                class="run-row"
                title={`${run.cmd ?? "Command not recorded"} ${run.target ?? run.id} — ${status.label}`}
                aria-label={`${run.cmd ?? "Command not recorded"} ${run.target ?? run.id}: ${status.label}`}
                class:selected={active}
                aria-current={active ? "true" : undefined}
                onclick={() =>
                  run.id === state.running
                    ? getSession().openConsole(run)
                    : getSession().selectRun(run.id)}
              >
                <span class="run-label"
                  ><RunStatus {...status} compact /><span class="run-command"
                    >{run.cmd ?? "?"}</span
                  >
                  {#if run.startedAt !== undefined}
                  <time datetime={new Date(run.startedAt).toISOString()}
                    >{new Date(run.startedAt).toLocaleTimeString([], {
                      hour: "2-digit",
                      minute: "2-digit",
                    })}</time
                  >{:else}<span>Time not recorded</span>{/if}</span
                >
                <span class="run-target">{run.target ?? run.id}</span>
              </button>
            </li>
          {/each}
        </ul>
        {#if app.view.runsError}<p class="error">{app.view.runsError}</p>
          <button type="button" class="button" onclick={getSession().loadRuns}
            >retry</button
          >
        {/if}
      </div>
    {/if}
  </section>
{/snippet}

{#snippet workspaceContent(state: AppState, workspace: string)}
  <main
    class="content"
    class:run-content={app.view.consoleOpen || !!app.view.selectedRun || analyticsOpen}
  >
    {#if app.view.console}
      {#key app.view.console.id}
        <div class="console-container" hidden={!app.view.consoleOpen}>
          {#await loadConsole() then { default: ConsoleView }}
          <ConsoleView record={app.view.runs.find((run) => run.id === app.view.console?.id) ?? app.view.console}
            visible={app.view.consoleOpen} running={state.running === app.view.console.id} stopping={state.stopping} />
          {/await}
        </div>
      {/key}
    {/if}
    {#if !app.view.consoleOpen}
    {#if snap?.meta.status === "partial"}
      <p class="warn">
        Skipped some packages (exit {state.lastError?.exitCode ?? "?"}) —
        partial results from
        {new Date(snap.meta.fetchedAt).toLocaleTimeString()}
      </p>
    {:else if snap && state.lastError}
      <p class="warn">
        Refresh failed — showing the snapshot from {new Date(
          snap.meta.fetchedAt,
        ).toLocaleTimeString()} ({state.lastError.summary})
      </p>
    {/if}
    {#if state.ingest === "running"}
      <p class="dim">Loading targets…</p>
    {/if}
    {#if app.view.selectedRun}
      {#key app.view.selectedRun}
        <RunView view={app.view} reload={getSession().loadDetail} reloadProfile={getSession().loadProfile} />
      {/key}
    {:else if analyticsOpen}
      {#key workspace}<AnalyticsView {workspace} runs={app.view.runs} />{/key}
    {:else if snap && selected}
      <TargetView
        {snap}
        label={selected}
        wsRoot={workspace}
        onselect={select}
      />
    {:else if snap}
      <p class="dim">Select a target in the tree</p>
    {:else if state.ingest !== "running" && state.lastError}
      <p class="error">Failed to load targets: {state.lastError.summary}</p>
      <button type="button" class="button" onclick={refresh}>retry</button>
    {/if}
    {/if}
  </main>
{/snippet}

<div class="win">
  <div class="toolbar">
    <div class="workspace-picker">
      <Cube size={22} weight="fill" class="brand-icon" />
      {#if st?.workspace}
        <button
          class="link label"
          title={st.workspace}
          disabled={busy}
          onclick={() => (switchingWs = !switchingWs)}
          >{st.workspace.split(/[\\/]/).filter(Boolean).at(-1) ??
            st.workspace}</button
        >
      {:else}
        <strong>Zeal</strong>
      {/if}
      {#if st?.workspace}
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
      {/if}
    </div>
    <div class="composer">
      <select class="select" aria-label="Bazel command" bind:value={cmd}>
        <option>build</option>
        <option>test</option>
        <option>run</option>
      </select>
      <div class="composer-search">
        <input
          class="input composer-target"
          aria-label="Target label or pattern"
          role="combobox"
          aria-expanded={dropdownOpen && !!snap}
          aria-controls={dropdownOpen && snap ? LIST_ID : undefined}
          aria-autocomplete="list"
          aria-activedescendant={activeOption}
          placeholder="Fuzzy :all / //pkg:target"
          autocomplete="off"
          value={targetField}
          oninput={(event) => {
            targetField = event.currentTarget.value;
            matchIndex = 0;
            dropdownOpen = targetField !== "";
          }}
          onkeydown={composerKey}
          onblur={() => (dropdownOpen = false)}
        />
        {#if dropdownOpen && snap}
          <SearchDropdown {matches} index={activeIndex} onpick={pickTarget} />
        {/if}
      </div>
      <button
        bind:this={argumentsToggle}
        type="button"
        class="button button-ghost arguments-toggle"
        aria-label={argumentsConfigured ? "Arguments (configured)" : "Arguments"}
        aria-expanded={argumentsOpen}
        aria-controls="run-arguments-row"
        title={argumentsConfigured ? argumentsText : "Specify arguments"}
        onclick={() => setArgumentsOpen(!argumentsOpen)}
      >
        Arguments
        {#if argumentsConfigured}<span class="arguments-indicator" aria-hidden="true"></span>{/if}
      </button>
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
    {#if argumentsOpen}
      <div class="arguments-row" id="run-arguments-row">
        <label for="run-arguments">Arguments</label>
        <input
          bind:this={argumentsInput}
          bind:value={argumentsText}
          id="run-arguments"
          class="input arguments-input"
          placeholder="--config=dev; run program arguments after --"
          title="Shell-style quotes and escapes. Quote # to keep it literal. No variable or command expansion."
          autocomplete="off"
          spellcheck={false}
          onkeydown={argumentsKey}
        />
      </div>
    {/if}
  </div>
  {#if app.view.error}<p class="app-error error" role="alert">
      {app.view.error}
    </p>{/if}
  {#if st?.runNote}<p class="app-error warn" role="status">{st.runNote}</p>{/if}
  {#if st?.besError}<p class="app-error warn" role="status">BES receiver: {st.besError}</p>{/if}
  {#if inputError}<p class="app-error error" role="status">{inputError}</p>{/if}

  {#if st === null}
    <div class="content dim">loading…</div>
  {:else if st.workspace === null || switchingWs}
    <div class="content">
      <WorkspacePrompt
        initial={st.workspace ?? ""}
        onclose={st.workspace ? () => (switchingWs = false) : undefined}
      />
      {#if st.workspace && st.bazel === null}
        <p class="error">bazelisk / bazel not found on PATH</p>
      {/if}
    </div>
  {:else}
    <div class="win-body">
      <aside class="sidebar">
        {@render targetPane(st)}
        <div
          class="divider"
          role="separator"
          aria-label="Resize target and run panes"
          aria-orientation="horizontal"
          onpointerdown={dividerDown}
        ></div>
        {@render runPane(st)}
      </aside>
      {@render workspaceContent(st, st.workspace)}
    </div>
  {/if}
</div>
