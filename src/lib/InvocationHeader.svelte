<script lang="ts">
  import type { Snippet } from "svelte";
  import type { BazelCommand, InvocationRecord, OwnedInvocation } from "./types";
  import { rerunArgs, type InvocationStatus as Status } from "./invocation";
  import InvocationStatus from "./InvocationStatus.svelte";

  let {
    record,
    status,
    execution,
    canRerun = false,
    onstop,
    onrerun,
    children,
  }: {
    record: InvocationRecord;
    status: Status;
    execution?: OwnedInvocation;
    canRerun?: boolean;
    onstop: (id: string) => void;
    onrerun?: (cmd: BazelCommand, pattern: string) => void;
    children?: Snippet;
  } = $props();

  const rerun = $derived(rerunArgs(record));
  const titleText = $derived(`${record.cmd ? `${record.cmd} ` : ""}${record.pattern ?? record.id}`);
</script>

<div class="invocation-header">
  <header class="invocation-summary">
    <InvocationStatus {...status} />
    <div class="invocation-heading">
      <h1 title={titleText}>{titleText}</h1>
      {#if execution}<p class="invocation-owner">Workspace {execution.workspace}</p>{/if}
    </div>
  </header>
  {@render children?.()}
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
      onclick={() => rerun && onrerun(rerun.cmd, rerun.pattern)}
    >
      Re-run
    </button>
  {/if}
</div>
