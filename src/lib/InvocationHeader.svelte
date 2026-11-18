<script lang="ts">
  import type { Snippet } from "svelte";
  import type { InvocationRecord, OwnedInvocation } from "./types";
  import type { InvocationStatus as Status } from "./invocation";
  import InvocationStatus from "./InvocationStatus.svelte";

  let { record, status, execution, onstop, children }: {
    record: InvocationRecord;
    status: Status;
    execution?: OwnedInvocation;
    onstop: (id: string) => void;
    children?: Snippet;
  } = $props();
</script>

<div class="invocation-header">
  <header class="invocation-summary">
    <InvocationStatus {...status} />
    <div class="invocation-heading">
      <h1>{#if record.cmd}{record.cmd}{" "}{/if}{record.pattern ?? record.id}</h1>
      {#if execution}<p class="invocation-owner">Workspace {execution.workspace}</p>{/if}
    </div>
  </header>
  {@render children?.()}
  {#if execution?.stoppable}
    <button
      type="button"
      class="button button-ghost"
      disabled={execution.stopping}
      aria-label={`${execution.stopping ? "Stopping" : "Stop"} in ${execution.workspace}`}
      onclick={() => onstop(execution.id)}
    >{execution.stopping ? "Stopping…" : "Stop"}</button>
  {/if}
</div>
