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
    <h1>{#if record?.cmd}{record.cmd}{" "}{/if}{record?.pattern ?? fallback ?? record?.id}</h1>
  </header>
  {@render children?.()}
</div>
