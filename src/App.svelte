<script lang="ts">
  import { onMount } from "svelte";
  import { app, connect, refresh, session } from "./lib/state.svelte";
  import { canSend, canStop } from "./lib/session";
  import { outcome } from "./lib/run";
  import { candidates, createIndex, keyAction, LIST_ID, moveIndex, optionId, search } from "./lib/search";
  import { targetError } from "./lib/target";
  import { buildTree } from "./lib/model";
  import Tree from "./lib/Tree.svelte";
  import TargetView from "./lib/TargetView.svelte";
  import WorkspacePrompt from "./lib/WorkspacePrompt.svelte";
  import RunView from "./lib/RunView.svelte";
  import SearchDropdown from "./lib/SearchDropdown.svelte";
  import type { AppState, RunCmd } from "./lib/types";

  onMount(connect);

  const st = $derived(app.state);
  const snap = $derived(st?.snapshot ?? null);
  const tree = $derived(snap ? buildTree(snap) : null);
  const known = $derived(new Set(snap?.targets.map((t) => t.label) ?? []));

  let selected = $state<string | null>(null);
  let targetField = $state("");
  let cmd = $state<RunCmd>("build");
  let switchingWs = $state(false);
  const busy = $derived(!app.view.ready || app.view.starting || app.view.opening || !!st?.running || st?.ingest === "running");
  const running = $derived(!!st?.running);
  const inputError = $derived(targetField === "" ? null : targetError(targetField));

  // The composer is the only search entry point. `dropdownOpen` is set by
  // typing only: programmatic insertion (tree, relations, run rows) replaces
  // the field without dropping the list over it. The matches are computed even
  // while closed, because ↑/↓ reopen the list from the current text.
  let dropdownOpen = $state(false);
  let matchIndex = $state(0);
  const index = $derived(snap ? createIndex(candidates(snap)) : null);
  const matches = $derived(index ? search(index, targetField) : []);
  // A snapshot swap can shorten the list under the selection, so clamp here:
  // the highlight, `aria-activedescendant` and Enter all name the same row.
  const activeIndex = $derived(moveIndex(matchIndex, 0, matches.length));
  const activeOption = $derived(
    dropdownOpen && matches.length > 0 ? optionId(activeIndex) : undefined,
  );

  function pickTarget(value: string) {
    targetField = value;
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
    if (running) void session.stop();
    else void session.start(cmd, targetField);
  }

  // Sidebar split: share of height given to the targets pane, and per-pane
  // collapse. The divider drags the boundary like VS Code's stacked views.
  let targetsShare = $state(0.65);
  let collapsed = $state({ targets: false, runs: false });

  function dividerDown(e: PointerEvent) {
    e.preventDefault();
    const sidebar = (e.currentTarget as HTMLElement).parentElement!;
    const move = (ev: PointerEvent) => {
      const r = sidebar.getBoundingClientRect();
      targetsShare = Math.min(0.85, Math.max(0.15, (ev.clientY - r.top) / r.height));
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
    session.selectRun(null);
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
      onclick={() => (collapsed.targets = !collapsed.targets)}
    >
      {collapsed.targets ? "▸" : "▾"} targets
    </button>
    {#if !collapsed.targets}
      <div class="pane-body">
        {#if snap && tree}
          <Tree root={tree} {selected} onselect={select} />
        {:else if state.ingest === "running"}
          <p class="dim">取り込み中…</p>
        {:else if state.lastError}
          <p class="err">{state.lastError.summary}</p>
          <button class="btn" onclick={refresh}>retry</button>
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
    <button
      class="pane-head"
      onclick={() => (collapsed.runs = !collapsed.runs)}
    >
      {collapsed.runs ? "▸" : "▾"} runs
    </button>
    {#if !collapsed.runs}
      <div class="pane-body">
        <ul class="list">
          {#each app.view.runs as run (run.id)}
            <li><button class="run-row" class:sel={app.view.selectedRun === run.id} aria-current={app.view.selectedRun === run.id ? "true" : undefined} onclick={() => session.selectRun(run.id)}>
              <span>{run.cmd} {run.target}</span>
              <small>{outcome(run, state.running, state.stopping)} · {new Date(run.startedAt).toLocaleTimeString()}</small>
            </button></li>
          {/each}
        </ul>
        {#if app.view.runsError}<p class="err">{app.view.runsError}</p><button class="btn" onclick={session.loadRuns}>retry</button>
        {:else if app.view.runs.length === 0}<p class="dim">Send で build / test を実行</p>{/if}
      </div>
    {/if}
  </section>
{/snippet}

{#snippet workspaceContent(state: AppState, workspace: string)}
  <main class="content" class:run-content={!!app.view.selectedRun}>
    {#if snap?.meta.status === "partial"}
      <p class="warn">
        一部の package を読み飛ばしました (exit {state.lastError?.exitCode ?? "?"}) —
        {new Date(snap.meta.fetchedAt).toLocaleTimeString()} の部分結果
      </p>
    {:else if snap && state.lastError}
      <p class="warn">
        更新に失敗 — {new Date(snap.meta.fetchedAt).toLocaleTimeString()} の
        snapshot を表示中 ({state.lastError.summary})
      </p>
    {/if}
    {#if state.ingest === "running"}
      <p class="dim">取り込み中…</p>
    {/if}
    {#if app.view.selectedRun}
      {#key app.view.selectedRun}
        <RunView view={app.view} reload={session.loadDetail} />
      {/key}
    {:else if snap && selected}
      <TargetView {snap} label={selected} wsRoot={workspace} onselect={select} />
    {:else if snap}
      <p class="dim">tree から target を選択</p>
    {:else if state.ingest !== "running" && state.lastError}
      <p class="err">取り込みに失敗しました: {state.lastError.summary}</p>
      <button class="btn" onclick={refresh}>retry</button>
    {/if}
  </main>
{/snippet}

<div class="win">
  <div class="toolbar">
    <strong>Zeal</strong>
    {#if st?.workspace}
      <button
        class="link label"
        title={st.workspace}
        disabled={busy}
        onclick={() => (switchingWs = !switchingWs)}>{st.workspace}</button
      >
    {:else}
      <span class="label">no workspace</span>
    {/if}
    <select class="select" aria-label="Bazel command" bind:value={cmd}>
      <option>build</option>
      <option>test</option>
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
        placeholder="//pkg:target"
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
      class="btn primary"
      disabled={switchingWs || (running ? !canStop(app.view) : !canSend(app.view, targetField))}
      onclick={submit}
    >
      {running ? "stop" : "Send"}
    </button>
    {#if st?.ingest === "running"}
      <span class="label" role="status">target データの取得待ち</span>
    {/if}
    {#if st?.workspace}
      <button class="btn" onclick={refresh} disabled={busy}>
        refresh
      </button>
    {/if}
  </div>
  {#if app.view.error}<p class="app-error err" role="alert">{app.view.error}</p>{/if}
  {#if st?.runNote}<p class="app-error warn" role="status">{st.runNote}</p>{/if}
  {#if inputError}<p class="app-error err" role="status">{inputError}</p>{/if}

  {#if st === null}
    <div class="content dim">loading…</div>
  {:else if st.workspace === null || switchingWs}
    <div class="content">
      <WorkspacePrompt
        initial={st.workspace ?? ""}
        onclose={st.workspace ? () => (switchingWs = false) : undefined}
      />
      {#if st.workspace && st.bazel === null}
        <p class="err">PATH に bazelisk / bazel が見つかりません</p>
      {/if}
    </div>
  {:else}
    <div class="win-body">
      <aside class="sidebar">
        {@render targetPane(st)}
        <div
          class="divider"
          role="separator"
          aria-orientation="horizontal"
          onpointerdown={dividerDown}
        ></div>
        {@render runPane(st)}
      </aside>
      {@render workspaceContent(st, st.workspace)}
    </div>
  {/if}
</div>
