<script lang="ts">
  import type { Snippet } from "svelte";
  import type { InvocationRecord } from "./types";
  import type { InvocationStatus as Status } from "./invocation";
  import InvocationStatus from "./InvocationStatus.svelte";

  let { record, fallback, status, children }: {
    record?: InvocationRecord;
    fallback?: string | null;
    status: Status;
    children?: Snippet;
  } = $props();
</script>

<div class="invocation-header">
  <header class="invocation-summary">
    <InvocationStatus {...status} />
    <div>
      <h1>{record?.pattern ?? fallback ?? record?.id}{#if record?.cmd}<span class="badge">{record.cmd}</span>{/if}</h1>
    </div>
  </header>
  {@render children?.()}
</div>

<style>

  h1 .badge {
    margin-left: 8px;
  }
</style>
