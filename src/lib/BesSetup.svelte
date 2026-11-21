<script lang="ts">
  import CopyButton from "./CopyButton.svelte";

  let { endpoint }: { endpoint: string | null } = $props();
  let copyError = $state("");
  const persistent = $derived(endpoint === "grpc://127.0.0.1:4718");
  const flag = $derived(`--bes_backend=${endpoint}`);
  const text = $derived(
    persistent
      ? `build ${flag}\nbuild --experimental_build_event_upload_strategy=local\nbuild --bes_timeout=5s\nbuild --experimental_collect_system_network_usage`
      : flag,
  );
</script>

{#if endpoint}
  <p class="scope-help">
    {#if persistent}To record CLI invocations, add these lines to your workspace's
    <code>.bazelrc</code> and keep Zeal open while running Bazel.{:else}Pass this
    option to Bazel for this Zeal session only. The port may change on
    restart.{/if}
  </p>
  <div class="query-command">
    <pre class="query-command-pre"><code>{text}</code></pre>
    <CopyButton
      {text}
      label={persistent ? "Copy .bazelrc settings" : "Copy session option"}
      onerror={(message) => (copyError = message)}
    />
  </div>
  {#if copyError}<p role="status">{copyError}</p>{/if}
{/if}
