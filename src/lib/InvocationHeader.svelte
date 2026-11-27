<script lang="ts">
  import type { Snippet } from "svelte";
  import type { BazelCommand, InvocationRecord, OwnedInvocation } from "./types";
  import type { InvocationStatus as Status, RerunArgs } from "./invocation";
  import InvocationStatus from "./InvocationStatus.svelte";

  let {
    record,
    status,
    execution,
    rerun = null,
    canRerun = false,
    onstop,
    onrerun,
    children,
  }: {
    record: InvocationRecord;
    status: Status;
    execution?: OwnedInvocation;
    rerun?: RerunArgs | null;
    canRerun?: boolean;
    onstop: (id: string) => void;
    onrerun?: (cmd: BazelCommand, pattern: string, flags: string) => void;
    children?: Snippet;
  } = $props();

  const titleText = $derived(`${record.cmd ? `${record.cmd} ` : ""}${record.pattern ?? record.id}`);
</script>

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
        <button
          type="button"
          class="button button-primary"
          disabled={!rerun || !canRerun}
          onclick={() => rerun && onrerun(rerun.cmd, rerun.pattern, rerun.flags)}
        >
          Re-run
        </button>
      {/if}
    </div>
  </div>
  {@render children?.()}
</div>
