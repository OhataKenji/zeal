<script lang="ts">
  import VizWorker from "./viz-worker?worker";
  import { onDestroy, untrack } from "svelte";
  import Notice from "../Notice.svelte";
  import { createGraphRenderer, type GraphState } from "./graph";

  let { dot }: { dot: string | null } = $props();

  // A successful graph invocation's DOT becomes an SVG blob URL; Graphviz output is
  // never appended to the DOM (CSP is unset and SVG links are an injection
  // surface).
  let graph = $state<GraphState>({ url: null, error: null, loading: false });
  const renderer = createGraphRenderer(
    () => new VizWorker(),
    (s) => (graph = s),
  );
  let graphZoom = $state(1);
  let graphDialog = $state<HTMLDialogElement>();
  let graphCanvas = $state<HTMLDivElement>();

  $effect(() => {
    renderer.render(dot);
  });
  $effect(() => {
    void dot;
    untrack(() => {
      graphZoom = 1;
      if (graphDialog?.open) graphDialog.close();
    });
  });
  onDestroy(() => renderer.dispose());
</script>

{#if dot !== null}
  {#if dot.trim() === ""}
    <p class="query-empty">No graph output.</p>
  {:else if graph.error}
    <Notice tone="error">{graph.error}</Notice>
    <pre class="query-raw">{dot}</pre>
  {:else if graph.url}
    <div class="query-graph-actions">
      <button
        type="button"
        class="button"
        onclick={() => {
          graphZoom = 1;
          graphDialog?.showModal();
          graphCanvas?.focus();
        }}>Open viewer</button
      >
    </div>
    <img
      class="query-graph"
      src={graph.url}
      alt="Dependency graph for the query"
    />
    <dialog
      class="query-graph-dialog"
      aria-label="Query dependency graph viewer"
      bind:this={graphDialog}
    >
      <div class="query-graph-toolbar">
        <span>Graph</span>
        <button
          type="button"
          class="button"
          aria-label="Zoom out"
          disabled={graphZoom <= 0.25}
          onclick={() => (graphZoom = Math.max(0.25, graphZoom - 0.25))}
          >−</button
        >
        <button
          type="button"
          class="button"
          title="Reset to original size"
          onclick={() => (graphZoom = 1)}
          >{Math.round(graphZoom * 100)}%</button
        >
        <button
          type="button"
          class="button"
          aria-label="Zoom in"
          disabled={graphZoom >= 4}
          onclick={() => (graphZoom = Math.min(4, graphZoom + 0.25))}
          >+</button
        >
        <button
          type="button"
          class="button"
          onclick={() => graphDialog?.close()}>Close</button
        >
      </div>
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (The scroll container needs focus for keyboard scrolling.) -->
      <div
        class="query-graph-canvas"
        role="region"
        aria-label="Graph canvas"
        tabindex="0"
        bind:this={graphCanvas}
      >
        <img
          src={graph.url}
          alt="Dependency graph for the query"
          style:zoom={graphZoom}
        />
      </div>
    </dialog>
  {:else}
    <p class="query-empty" role="status">Rendering graph…</p>
  {/if}
{/if}
