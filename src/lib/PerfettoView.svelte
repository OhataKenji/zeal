<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { COMMANDS } from "./commands";
  import { startTraceHandshake } from "./perfetto";
  import Notice from "./Notice.svelte";

  let {
    workspace,
    id,
    title,
  }: { workspace: string; id: string; title?: string } = $props();

  let frame = $state<HTMLIFrameElement>();
  let viewer = $state<string | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(true);
  let attempt = $state(0);

  // The parent keys this component by workspace+id; `attempt` only bumps on
  // manual retry. Cleanup guarantees no send outlives the view.
  $effect(() => {
    void attempt;
    let cancelled = false;
    let stop: (() => void) | null = null;
    void (async () => {
      try {
        const origin = await invoke<string>(COMMANDS.perfettoOrigin);
        if (cancelled) return;
        viewer = origin;
        stop = startTraceHandshake({
          frame: () => frame,
          viewer: origin,
          load: () =>
            invoke<ArrayBuffer>(COMMANDS.readInvocationTrace, { workspace, id }),
          title: title ?? `Invocation ${id}`,
          onSent: () => {
            loading = false;
          },
          onError: (failure) => {
            const detail = failure as { type?: string; message?: string } | null;
            error =
              detail?.type === "missing"
                ? "No saved timing profile is available for this invocation."
                : `Could not load timing data: ${detail?.message ?? String(failure)}`;
            loading = false;
          },
          onTimeout: () => {
            error = "The timing viewer did not respond.";
            loading = false;
          },
        });
      } catch (failure) {
        if (!cancelled) {
          error = `Could not load timing data: ${String(failure)}`;
          loading = false;
        }
      }
    })();
    return () => {
      cancelled = true;
      stop?.();
    };
  });

  const retry = () => {
    error = null;
    loading = true;
    viewer = null;
    attempt += 1;
  };
</script>

<div class="perfetto-view">
  {#if error}
    <Notice tone="error" onaction={retry}>{error}</Notice>
  {:else}
    {#if loading}<p class="panel-empty" role="status">Loading timing viewer…</p>{/if}
    <iframe
      bind:this={frame}
      src={viewer === null ? undefined : `${viewer}/#!/?mode=embedded`}
      sandbox="allow-scripts allow-same-origin"
      title="Invocation timing"
    ></iframe>
  {/if}
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
