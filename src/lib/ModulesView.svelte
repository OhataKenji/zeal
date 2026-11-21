<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onDestroy, untrack } from "svelte";
  import Notice from "./Notice.svelte";
  import {
    createModules,
    emptyModulesState,
    filterModules,
    type ModuleDepth,
    type ModulesState,
  } from "./modules";

  let {
    workspace,
    bazelAvailable,
    initialState = emptyModulesState(),
  }: {
    workspace: string;
    bazelAvailable: boolean;
    initialState?: ModulesState;
  } = $props();

  const view = $state(untrack(() => ({ ...initialState })));
  let depth = $state<ModuleDepth>("all");
  let query = $state("");
  const snapshot = $derived(view.snapshot);
  const selected = $derived(snapshot?.modules.find((node) => node.key === view.selectedKey));
  const disabled = $derived(!bazelAvailable || view.busy !== null);
  const controller = createModules(
    untrack(() => workspace),
    { invoke },
    view,
    () => bazelAvailable,
  );
  onDestroy(controller.dispose);
</script>

{#snippet relations(title: string, keys: string[])}
  <section class="section">
    <h3 class="section-title">{title}</h3>
    <ul class="list mono">
      {#each keys as key (key)}
        <li>
          {#if key === snapshot?.rootKey}
            {key}
          {:else}
            <button type="button" class="table-link" onclick={() => controller.select(key)}>{key}</button>
          {/if}
        </li>
      {:else}
        <li class="dim">None</li>
      {/each}
    </ul>
  </section>
{/snippet}

<main class="content">
  <div class="page-center">
    <header class="view-head">
      <div>
        <h1 class="view-title">Modules</h1>
        <div class="view-meta">Explore resolved Bzlmod dependencies and their selected versions.</div>
      </div>
      {#if snapshot}
        <button type="button" class="button" {disabled} onclick={controller.load}>
          {view.busy === "load" ? "Refreshing…" : "Refresh"}
        </button>
      {/if}
    </header>
    {#if !bazelAvailable}
      <Notice tone="neutral">Bazel is unavailable. Install Bazel or Bazelisk to load modules.</Notice>
    {/if}
    {#if view.loadError !== null}
      <Notice tone={snapshot ? "warning" : "error"} onaction={disabled ? undefined : controller.load}>
        {view.loadError}
      </Notice>
    {/if}
    {#if !snapshot}
      {#if view.loadError === null}
        <p>
          Run <code>bazel mod graph</code> to load dependencies.
          With a cold cache, Bazel may evaluate module extensions.
        </p>
        <button type="button" class="button button-primary" {disabled} onclick={controller.load}>
          {view.busy === "load" ? "Loading modules…" : "Load modules"}
        </button>
      {/if}
    {:else}
      <div class="module-filters">
        <input
          class="input"
          type="search"
          aria-label="Search modules"
          placeholder="Search modules…"
          bind:value={query}
        />
        <select class="input" aria-label="Dependency depth" bind:value={depth}>
          <option value="all">All</option>
          <option value="direct">Direct</option>
          <option value="transitive">Transitive</option>
        </select>
      </div>
      <table class="lens-table">
        <thead><tr><th>Module</th><th>Version</th><th>Depth</th><th>Required by</th></tr></thead>
        <tbody>
          {#each filterModules(snapshot.modules, query, depth) as node (node.key)}
            <tr
              class="clickable"
              class:selected={node.key === view.selectedKey}
              onclick={(e) => {
                if (!(e.target as HTMLElement).closest("button")) controller.select(node.key);
              }}
            >
              <td>
                <button
                  type="button"
                  class="table-link mono"
                  aria-pressed={node.key === view.selectedKey}
                  onclick={() => controller.select(node.key)}
                >{node.name}</button>
              </td>
              <td class="mono">{node.version || "—"}</td>
              <td>{node.direct ? "direct" : "transitive"}</td>
              <td>{node.requiredBy.length}</td>
            </tr>
          {:else}
            <tr><td colspan="4" class="dim">No modules to display.</td></tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
</main>

{#if selected}
  <aside class="inspector" aria-label="Module details">
    <header class="view-head">
      <div>
        <h2 class="view-title mono">{selected.key}</h2>
        <span class="badge">{selected.direct ? "direct" : "transitive"}</span>
      </div>
      <button
        type="button"
        class="button button-ghost icon-button"
        aria-label="Close"
        onclick={() => controller.select(null)}>✕</button
      >
    </header>
    {@render relations("Required by", selected.requiredBy)}
    {@render relations("Dependencies", selected.dependencies)}
    <section class="section">
      <div class="section-head">
        <h3 class="section-title">Explain (MVS)</h3>
        <button type="button" class="button" {disabled} onclick={controller.explain}>
          {view.busy === "explain" ? "Explanation in progress…" : "Explain"}
        </button>
      </div>
      {#if view.explainError !== null}
        <Notice tone="error" onaction={disabled ? undefined : controller.explain}>
          {view.explainError}
        </Notice>
      {:else if view.explanation !== null}
        <pre class="code-box">{view.explanation}</pre>
      {/if}
    </section>
  </aside>
{/if}

<style>
  .module-filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
  }
  select.input {
    flex: none;
    width: auto;
  }
  input.input {
    flex: 1 1 12em;
  }
</style>
