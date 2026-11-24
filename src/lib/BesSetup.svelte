<script lang="ts">
  import CopyButton from "./CopyButton.svelte";

  let { endpoint }: { endpoint: string | null } = $props();

  const DEFAULT_ENDPOINT = "grpc://127.0.0.1:4718";
  const BAZELRC = `build --bes_backend=${DEFAULT_ENDPOINT}
build --experimental_build_event_upload_strategy=local
build --bes_timeout=5s
build --experimental_collect_system_network_usage`;
</script>

{#snippet copyBlock(text: string, label: string)}
  <div class="query-command">
    <pre class="query-command-pre"><code>{text}</code></pre>
    <CopyButton {text} {label} />
  </div>
{/snippet}
<p class="scope-help">Add to your workspace's <code>.bazelrc</code> to record CLI invocations:</p>
{@render copyBlock(BAZELRC, "Copy .bazelrc settings")}
{#if endpoint && endpoint !== DEFAULT_ENDPOINT}
  <p class="scope-help">
    Temporary port: pass this flag on each Bazel command this session, not in <code>.bazelrc</code>.
  </p>
  {@render copyBlock(`--bes_backend=${endpoint}`, "Copy session option")}
{/if}
