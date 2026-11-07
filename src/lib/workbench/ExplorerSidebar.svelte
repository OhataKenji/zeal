<script lang="ts">
  import { CaretDown, CaretRight, ChartBar, TreeStructure } from "phosphor-svelte";
  import { onDestroy } from "svelte";
  import Notice from "../Notice.svelte";
  import Tree from "../Tree.svelte";
  import InvocationStatus from "../InvocationStatus.svelte";
  import { invocationStatus } from "../invocation";
  import { buildTree } from "../model";
  import type {
    AppState,
    ExplorerSelection,
    InvocationRecord,
    TargetDetail,
    TargetIndex,
  } from "../types";

  let {
    visible,
    state: core,
    targets,
    history,
    page,
    revealRequest,
    onselect,
    oninvocation,
    onpage,
    onretryTargets,
    onretryInvocations,
    onTargetVisibility,
  }: {
    visible: boolean;
    state: AppState | null;
    targets: {
      snapshot: TargetIndex | null;
      snapshotLoading: boolean;
      snapshotError: string | null;
      selection: ExplorerSelection;
      targetDetail: TargetDetail | null;
      targetDetailError: string | null;
    };
    history: {
      records: InvocationRecord[];
      error: string | null;
      selectedId: string | null;
      consoleId: string | null;
    };
    page: "explorer" | "query" | "analytics" | "invocation" | "console";
    revealRequest: number;
    onselect: (selection: ExplorerSelection) => void;
    oninvocation: (record: InvocationRecord) => void;
    onpage: (page: "query" | "analytics") => void;
    onretryTargets: () => void;
    onretryInvocations: () => void;
    onTargetVisibility: (visible: boolean) => void;
  } = $props();

  const tree = $derived(targets.snapshot ? buildTree(targets.snapshot) : null);

  // Sidebar split: share of height given to the targets pane, and per-pane
  // collapse. The divider drags the boundary like VS Code's stacked views.
  let targetsShare = $state(0.55);
  let collapsed = $state({ targets: false, invocations: false });
  let endDrag: (() => void) | undefined;

  function dividerDown(e: PointerEvent) {
    e.preventDefault();
    const divider = e.currentTarget;
    if (!(divider instanceof HTMLElement) || !divider.parentElement) return;
    const sidebar = divider.parentElement;
    endDrag?.();
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
      endDrag = undefined;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    endDrag = up;
  }

  onDestroy(() => endDrag?.());

  $effect(() => {
    if (!visible) endDrag?.();
  });

  $effect(() => {
    if (revealRequest > 0) collapsed.targets = false;
  });

  $effect(() => {
    onTargetVisibility(!collapsed.targets);
  });
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
          >{targets.snapshot?.targets.length.toLocaleString() ?? "—"}</span
        >
      </button>
      <button
        class="analytics-link"
        class:active={page === "query"}
        onclick={() => onpage("query")}><TreeStructure size={12} /> Query</button
      >
    </div>
    {#if !collapsed.targets}
      <div class="pane-body">
        {#if targets.snapshot && tree}
          <Tree
            root={tree}
            selection={targets.selection}
            {revealRequest}
            {onselect}
          />
        {:else if state.ingest === "running" || targets.snapshotLoading}
          <p class="dim">Loading targets…</p>
        {:else if targets.snapshotError || state.lastError}
          <Notice tone="error" onaction={onretryTargets}
            >{targets.snapshotError ?? state.lastError?.summary}</Notice
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
        class:active={page === "analytics"}
        onclick={() => onpage("analytics")}><ChartBar size={12} /> Analytics</button
      >
    </div>
    {#if !collapsed.invocations}
      <div class="pane-body">
        <ul class="list">
          {#each history.records as inv (inv.id)}
            {@const status = invocationStatus(inv, state.activeInvocation, state.stopping)}
            {@const active =
              history.selectedId === inv.id || history.consoleId === inv.id}
            <li>
              <button
                class="invocation-row"
                title={`${inv.cmd ?? "Command not recorded"} ${inv.pattern ?? inv.id} — ${status.label}`}
                aria-label={`${inv.cmd ?? "Command not recorded"} ${inv.pattern ?? inv.id}: ${status.label}`}
                class:selected={active}
                aria-current={active ? "true" : undefined}
                onclick={() => oninvocation(inv)}
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
        {#if history.error}<Notice tone="error" onaction={onretryInvocations}
            >{history.error}</Notice
          >
        {/if}
      </div>
    {/if}
  </section>
{/snippet}

{#if visible && core}
  <aside class="sidebar">
    {@render targetPane(core)}
    <div
      class="divider"
      role="separator"
      aria-label="Resize target and invocation panes"
      aria-orientation="horizontal"
      onpointerdown={dividerDown}
    ></div>
    {@render invocationPane(core)}
  </aside>
{/if}
