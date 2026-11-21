<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onDestroy, untrack } from "svelte";
  import CopyButton from "./CopyButton.svelte";
  import Notice from "./Notice.svelte";
  import {
    createModules,
    emptyModulesState,
    filterModules,
    filterRepos,
    type ModuleDepth,
    type ModulesState,
  } from "./modules";

  const tabs = ["Modules", "Root Repos"] as const;
  type Tab = (typeof tabs)[number];
  let {
    workspace,
    bazelAvailable,
    initialState = emptyModulesState(),
    initialTab = "Modules",
  }: {
    workspace: string;
    bazelAvailable: boolean;
    initialState?: ModulesState;
    initialTab?: Tab;
  } = $props();

  let view = $state.raw(untrack(() => initialState));
  let tab = $state<Tab>(untrack(() => initialTab));
  let depth = $state<ModuleDepth>("all");
  let query = $state("");
  let copyError = $state("");
  const snapshot = $derived(view.snapshot);
  const selected = $derived(snapshot?.modules.find((node) => node.key === view.selectedKey));
  const disabled = $derived(!bazelAvailable || view.busy !== null);
  const controller = createModules(
    untrack(() => workspace),
    { invoke },
    (next) => (view = next),
    () => bazelAvailable,
  );
  onDestroy(controller.dispose);

  function showTab(next: Tab) {
    if (next === tab) return;
    tab = next;
    query = "";
    copyError = "";
    controller.select(null);
  }
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
            <button class="module-link" onclick={() => controller.select(key)}>{key}</button>
          {/if}
        </li>
      {:else}
        <li class="dim">None</li>
      {/each}
    </ul>
  </section>
{/snippet}

{#snippet repoName(name: string)}
  <div class="repo-name mono">
    {name}
    <CopyButton text={name} label={`Copy ${name}`} onerror={(error) => (copyError = error)} />
  </div>
{/snippet}

<main class="content">
  <div class="page-center">
    <header class="view-head">
      <div>
        <h1 class="view-title">Modules</h1>
        <div class="view-meta">Resolved Bzlmod dependencies and repositories visible from the root.</div>
      </div>
      {#if snapshot}
        <button class="button" {disabled} onclick={controller.load}>
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
        <button class="button button-primary" {disabled} onclick={controller.load}>
          {view.busy === "load" ? "Loading modules…" : "Load modules"}
        </button>
      {/if}
    {:else}
      <div class="module-filters">
        <div class="sub-switch" role="group" aria-label="Module views">
          {#each tabs as item (item)}
            <button
              class:active={tab === item}
              aria-pressed={tab === item}
              onclick={() => showTab(item)}
            >
              {item}
            </button>
          {/each}
        </div>
        <input
          class="input"
          type="search"
          aria-label={`Search ${tab}`}
          placeholder={`Search ${tab}…`}
          bind:value={query}
        />
        {#if tab === "Modules"}
          <select class="input" aria-label="Dependency depth" bind:value={depth}>
            <option value="all">All</option>
            <option value="direct">Direct</option>
            <option value="transitive">Transitive</option>
          </select>
        {/if}
      </div>
      {#if tab === "Modules"}
        <table class="lens-table">
          <thead><tr><th>Module</th><th>Version</th><th>Depth</th><th>Required by</th></tr></thead>
          <tbody>
            {#each filterModules(snapshot.modules, query, depth) as node (node.key)}
              <tr class:selected={node.key === view.selectedKey}>
                <td>
                  <button
                    class="module-link mono"
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
      {:else}
        {#if snapshot.repoMappingError !== null}
          <Notice tone="warning">{snapshot.repoMappingError}</Notice>
        {/if}
        {#if copyError}<Notice tone="warning">{copyError}</Notice>{/if}
        <table class="lens-table">
          <thead><tr><th>Apparent</th><th>Canonical</th></tr></thead>
          <tbody>
            {#each filterRepos(snapshot.repos, query) as repo (repo.apparent)}
              <tr>
                <td>{@render repoName(`@${repo.apparent}`)}</td>
                <td>{@render repoName(`@@${repo.canonical}`)}</td>
              </tr>
            {:else}
              <tr><td colspan="2" class="dim">No repositories to display.</td></tr>
            {/each}
          </tbody>
        </table>
      {/if}
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
      <button class="button" onclick={() => controller.select(null)}>Close</button>
    </header>
    {@render relations("Required by", selected.requiredBy)}
    {@render relations("Dependencies", selected.dependencies)}
    <section class="section">
      <div class="section-head">
        <h3 class="section-title">Explain (MVS)</h3>
        <button class="button" {disabled} onclick={controller.explain}>
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
  .sub-switch,
  select.input {
    flex: none;
    width: auto;
  }
  input.input {
    flex: 1 1 12em;
  }
  .module-link {
    border: 0;
    background: transparent;
    padding: 2px 0;
    color: var(--primary);
    overflow-wrap: anywhere;
    text-align: left;
  }
  .module-link:hover {
    text-decoration: underline;
  }
  .repo-name {
    position: relative;
    padding: 10px 38px 10px 0;
    overflow-wrap: anywhere;
  }
</style>
