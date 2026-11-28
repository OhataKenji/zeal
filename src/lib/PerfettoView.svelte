<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { COMMANDS } from "./commands";
  import Notice from "./Notice.svelte";

  let { workspace, id }: { workspace: string; id: string } = $props();

  // The parent keys this component by workspace+id; `attempt` only bumps on
  // manual retry, so prepare runs once per mount or retry.
  let attempt = $state(0);
  const viewer = $derived.by(() => {
    void attempt;
    return invoke<string>(COMMANDS.prepareInvocationTiming, { workspace, id });
  });
</script>

<div class="perfetto-view">
  {#await viewer}
    <p class="panel-empty" role="status">Loading timing viewer…</p>
  {:then src}
    <iframe
      src={src}
      sandbox="allow-scripts allow-same-origin"
      title="Invocation timing"
    ></iframe>
  {:catch error}
    <Notice tone="error" onaction={() => attempt++}>{String(error)}</Notice>
  {/await}
</div>

<style>
  .perfetto-view {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 480px;
  }
  iframe {
    flex: 1;
    width: 100%;
    border: 0;
    min-height: 480px;
  }
</style>
