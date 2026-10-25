<script lang="ts">
  import type { Snippet } from "svelte";
  import type { RunRecord } from "./types";
  import type { RunStatus as Status } from "./run";
  import RunStatus from "./RunStatus.svelte";

  let { record, fallback, status, children }: {
    record?: RunRecord;
    fallback?: string | null;
    status: Status;
    children?: Snippet;
  } = $props();
</script>

<div class="run-header">
  <header class="run-summary">
    <RunStatus {...status} />
    <div>
      <h1>{record?.target ?? fallback}{#if record}<span class="badge">bazel {record.cmd}</span>{/if}</h1>
    </div>
  </header>
  {@render children?.()}
</div>

<style>
  h1 .badge { margin-left: 8px; }
</style>
