<script lang="ts">
  import type { Snippet } from "svelte";
  import type { BazelCommand, InvocationRecord, OwnedInvocation } from "./types";
  import { rerunArgs, type InvocationStatus as Status } from "./invocation";
  import InvocationStatus from "./InvocationStatus.svelte";

  let {
    record,
    status,
    execution,
    explicitCmdLine,
    canRerun = false,
    onstop,
    onrerun,
    onopenflags,
    children,
  }: {
    record: InvocationRecord;
    status: Status;
    execution?: OwnedInvocation;
    explicitCmdLine?: readonly string[];
    canRerun?: boolean;
    onstop: (id: string) => void;
    onrerun?: (cmd: BazelCommand, pattern: string, flags: string) => void;
    onopenflags?: (cmd: BazelCommand, pattern: string, flags: string) => void;
    children?: Snippet;
  } = $props();

  let menu = $state<HTMLDetailsElement>();
  const rerun = $derived(rerunArgs(record, explicitCmdLine));
  const rerunDisabled = $derived(!rerun || !canRerun);
  const rerunTitle = $derived(
    rerun
      ? `Re-run: bazel ${rerun.cmd} ${rerun.pattern}${rerun.flags ? ` ${rerun.flags}` : ""}`
      : undefined,
  );
  const titleText = $derived(`${record.cmd ? `${record.cmd} ` : ""}${record.pattern ?? record.id}`);

  function closeMenu() {
    if (menu) menu.open = false;
  }

  function onWindowPointerDown(event: PointerEvent) {
    if (menu?.open && !menu.contains(event.target as Node)) closeMenu();
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="invocation-header">
  <div class="invocation-head-row">
    <InvocationStatus {...status} />
    <h1 class="invocation-title" title={titleText}>{titleText}</h1>
    {#if execution}<p class="invocation-owner">Workspace {execution.workspace}</p>{/if}
    <div class="invocation-head-actions">
      {#if execution?.stoppable}
        <button
          type="button"
          class="button button-danger icon-button"
          disabled={execution.stopping}
          title={execution.stopping ? "Stopping…" : "Stop"}
          aria-label={`${execution.stopping ? "Stopping" : "Stop"} in ${execution.workspace}`}
          onclick={() => onstop(execution.id)}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <rect x="1" y="1" width="8" height="8" rx="1.2" fill="currentColor" />
          </svg>
        </button>
      {:else if !execution && onrerun}
        <div class="action-pair">
          <button
            type="button"
            class="button button-primary"
            disabled={rerunDisabled}
            title={rerunTitle}
            onclick={() => rerun && onrerun(rerun.cmd, rerun.pattern, rerun.flags)}
          >
            Re-run
          </button>
          {#if onopenflags}
            <details class="dd" bind:this={menu}>
              <summary class="button button-ghost" aria-label="More re-run actions">⋯</summary>
              <div class="dd-menu dd-menu-right dd-menu-compact">
                <button
                  type="button"
                  class="menu-item"
                  disabled={rerunDisabled}
                  onclick={() => {
                    closeMenu();
                    if (rerun) onopenflags(rerun.cmd, rerun.pattern, rerun.flags);
                  }}
                >
                  Re-run with flags…
                </button>
              </div>
            </details>
          {/if}
        </div>
      {/if}
    </div>
  </div>
  {@render children?.()}
</div>
