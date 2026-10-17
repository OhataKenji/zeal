<script lang="ts">
  import type { RunRecord } from "../types";

  let { record }: { record: RunRecord | undefined } = $props();
</script>

<h2 class="panel-heading">Run details</h2>
{#if record}
  <dl class="run-details">
    <dt>Command</dt><dd><code>bazel {record.cmd} {record.target}</code></dd>
    <dt>Workspace</dt><dd><code>{record.workspace}</code></dd>
    <dt>Run ID</dt><dd><code>{record.id}</code></dd>
    <dt>Started</dt><dd>{new Date(record.startedAt).toLocaleString()}</dd>
    <dt>Finished</dt><dd>{record.finishedAt === undefined ? "Not recorded" : new Date(record.finishedAt).toLocaleString()}</dd>
    <dt>Exit code</dt><dd>{record.exitCode ?? "Unknown"}</dd>
    {#if record.finishedAt !== undefined}<dt>Wall time</dt><dd>{((record.finishedAt - record.startedAt) / 1000).toFixed(3)}s</dd>{/if}
  </dl>
{/if}
