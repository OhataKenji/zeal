<script lang="ts">
  import { untrack } from "svelte";
  import CopyButton from "./CopyButton.svelte";
  import Notice from "./Notice.svelte";
  import type { TargetScope, WorkspaceInfo } from "./types";

  let {
    workspace,
    scope,
    info,
    loading,
    error,
    onretry,
    onsavescope,
  }: {
    workspace: string;
    scope: TargetScope;
    info: WorkspaceInfo | null;
    loading: boolean;
    error: string | null;
    onretry: () => void;
    onsavescope: (scope: TargetScope) => Promise<boolean>;
  } = $props();

  const name = $derived(workspace.split(/[\\/]/).filter(Boolean).at(-1) ?? workspace);
  let includeText = $state(untrack(() => scope.include.join("\n")));
  let saving = $state(false);
  let saveError = $state<string | null>(null);
  let copiedKey = $state<string | null>(null);
  let copyError = $state("");

  $effect(() => {
    includeText = scope.include.join("\n");
  });

  const rows = $derived.by(() => {
    const list: Array<{ key: string; path: string | null }> = [
      { key: "workspace", path: info?.workspace ?? workspace },
      { key: "output_base", path: info?.outputBase ?? null },
      { key: "execution_root", path: info?.executionRoot ?? null },
      { key: "bazel-bin", path: info?.bazelBin ?? null },
      { key: "bazel-testlogs", path: info?.bazelTestlogs ?? null },
      { key: "repository_cache", path: info?.repositoryCache ?? null },
    ];
    return list;
  });

  const BES_SNIPPET = `build --bes_backend=grpc://127.0.0.1:4718
build --experimental_build_event_upload_strategy=local
build --bes_timeout=5s
build --experimental_collect_system_network_usage`;

  async function copyPath(key: string, value: string) {
    try {
      await navigator.clipboard.writeText(value);
      copiedKey = key;
      setTimeout(() => {
        if (copiedKey === key) copiedKey = null;
      }, 1500);
    } catch {
      // Ignore clipboard failures in restricted environments.
    }
  }

  async function saveScope(event: SubmitEvent) {
    event.preventDefault();
    if (saving || includeText.trim() === "") return;
    saving = true;
    saveError = null;
    try {
      await onsavescope({ include: includeText.split("\n") });
    } catch (e) {
      saveError = String(e);
    } finally {
      saving = false;
    }
  }
</script>

<div class="view-head">
  <div>
    <h1 class="view-title mono">{name}</h1>
    <div class="view-meta mono">
      {workspace}{#if info?.release} · {info.release}{/if}
    </div>
  </div>
  <button type="button" class="button" disabled={loading} onclick={onretry}>
    {loading ? "Loading…" : "Refresh info"}
  </button>
</div>

<section class="section">
  <div class="section-head">
    <h2 class="section-title">Directories &amp; Caches (bazel info)</h2>
  </div>
  {#if error}
    <Notice tone="warning" onaction={onretry}>Could not read bazel info: {error}</Notice>
  {/if}
  <table class="lens-table">
    <thead>
      <tr><th>Key</th><th>Path</th></tr>
    </thead>
    <tbody>
      {#each rows as row (row.key)}
        <tr
          class:clickable={!!row.path}
          onclick={() => row.path && void copyPath(row.key, row.path)}
          title={row.path ? "Click to copy path" : undefined}
        >
          <td class="mono">{row.key}</td>
          <td class="mono path-cell">
            {#if row.path}
              {row.path}{#if copiedKey === row.key} <span class="copied-tag">Copied</span>{/if}
            {:else if loading}
              <span class="dim">Loading…</span>
            {:else}
              <span class="dim">—</span>
            {/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</section>

<section class="section">
  <div class="section-head">
    <h2 class="section-title">Target Discovery Scope</h2>
  </div>
  <form class="scope-form" onsubmit={saveScope}>
    <textarea
      class="input mono scope-input"
      rows="3"
      aria-label="Target discovery patterns"
      spellcheck={false}
      bind:value={includeText}
      disabled={saving}
    ></textarea>
    {#if saveError}<Notice tone="error">{saveError}</Notice>{/if}
    <div>
      <button type="submit" class="button" disabled={saving || includeText.trim() === ""}>
        {saving ? "Saving…" : "Save scope"}
      </button>
    </div>
  </form>
</section>

<section class="section">
  <div class="section-head">
    <h2 class="section-title">Local BES Receiver · 127.0.0.1:4718</h2>
    <CopyButton
      text={BES_SNIPPET}
      label="Copy .bazelrc snippet"
      onerror={(msg) => (copyError = msg)}
    />
  </div>
  {#if copyError}<Notice tone="error">{copyError}</Notice>{/if}
  <pre class="code-box">{BES_SNIPPET}</pre>
</section>
