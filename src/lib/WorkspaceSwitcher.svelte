<script lang="ts">
  import { onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { Check } from "phosphor-svelte";
  import { COMMANDS } from "./commands";
  import Notice from "./Notice.svelte";

  let {
    workspace,
    opening,
    onswitch,
    onopennew,
  }: {
    workspace: string | null;
    opening: boolean;
    onswitch: (path: string) => void;
    onopennew: () => void;
  } = $props();

  let details: HTMLDetailsElement | undefined;
  let summary: HTMLElement | undefined;
  let entries = $state.raw<string[] | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let disposed = false;

  const displayName = $derived(
    workspace?.split(/[\\/]/).filter(Boolean).at(-1) ?? "Zeal",
  );

  async function load() {
    if (loading) return;
    loading = true;
    error = null;
    try {
      const list = await invoke<string[]>(COMMANDS.listWorkspaces);
      if (!disposed) entries = list;
    } catch (e) {
      if (!disposed) error = String(e);
    } finally {
      if (!disposed) loading = false;
    }
  }

  function onToggle() {
    if (details?.open) void load();
  }

  function onKeydown(event: KeyboardEvent) {
    if (
      event.key !== "Escape" ||
      event.isComposing ||
      !details?.open ||
      !details.contains(document.activeElement)
    )
      return;
    event.preventDefault();
    details.open = false;
    summary?.focus();
  }

  function onWindowPointerDown(event: PointerEvent) {
    if (details?.open && !details.contains(event.target as Node)) {
      details.open = false;
    }
  }

  function choose(path: string) {
    if (details) details.open = false;
    onswitch(path);
  }

  function navigate(action: () => void) {
    if (details) details.open = false;
    action();
  }

  onDestroy(() => {
    disposed = true;
  });
</script>

<svelte:window onpointerdown={onWindowPointerDown} onkeydown={onKeydown} />

<details bind:this={details} class="ws-details" ontoggle={onToggle}>
  <summary
    bind:this={summary}
    class="ws-trigger"
    aria-label="Switch workspace"
    title={workspace ?? "Switch workspace"}
  >
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="76 76 360 360"
      width="18"
      height="18"
      class="zeal-logo"
      aria-hidden="true"
    >
      <polygon points="84,104 428,104 428,216 276,216 164,304 84,304" fill="#34d399" />
      <polygon points="428,208 348,208 236,296 84,296 84,408 428,408" fill="#0891b2" />
    </svg>
    <span class="ws-name">{displayName}</span>
    <span class="ws-caret" aria-hidden="true">▾</span>
  </summary>
  <div class="ws-menu">
    {#if error !== null}
      <Notice tone="error" onaction={load}>{error}</Notice>
    {/if}
    {#if loading && entries === null}
      <p class="dim ws-note">Loading workspaces…</p>
    {:else if entries !== null}
      <div class="ws-list">
        {#each entries as path (path)}
          <button
            type="button"
            class="ws-entry"
            disabled={opening || path === workspace}
            aria-current={path === workspace ? "true" : undefined}
            title={path}
            onclick={() => choose(path)}
          >
            <span class="ws-entry-path">{path}</span>
            {#if path === workspace}<Check size={12} />{/if}
          </button>
        {/each}
        {#if entries.length === 0}
          <p class="dim ws-note">No recent workspaces.</p>
        {/if}
      </div>
    {/if}
    <div class="ws-footer">
      <button
        type="button"
        class="button ws-open"
        onclick={() => navigate(onopennew)}
        disabled={opening}
      >
        Open workspace…
      </button>
    </div>
  </div>
</details>

<style>
  .ws-details {
    position: relative;
    display: flex;
    min-width: 0;
    max-width: 240px;
    flex: 0 1 auto;
  }

  .ws-trigger {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    min-width: 0;
    height: 26px;
    padding: 0 8px 0 6px;
    border: none;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--foreground);
    font-size: 12.5px;
    font-weight: 600;
    list-style: none;
    cursor: pointer;
    text-align: left;
  }

  .ws-trigger::-webkit-details-marker {
    display: none;
  }

  .ws-trigger:hover {
    background: var(--accent);
  }

  .zeal-logo {
    flex-shrink: 0;
  }

  .ws-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ws-caret {
    flex-shrink: 0;
    color: var(--muted-foreground);
    font-size: 10px;
  }

  .ws-menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 20;
    display: flex;
    width: min(420px, calc(100vw - 24px));
    max-height: min(70vh, 520px);
    flex-direction: column;
    gap: 8px;
    overflow: auto;
    padding: 8px;
    border: 1px solid var(--hairline);
    border-radius: var(--radius-md);
    background: var(--surface-panel);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.35);
  }

  .ws-list {
    display: flex;
    flex-direction: column;
  }

  .ws-entry {
    display: flex;
    width: 100%;
    align-items: center;
    gap: 8px;
    border: 0;
    border-radius: var(--radius-md);
    background: none;
    padding: 6px 8px;
    color: var(--foreground);
    font: 12px var(--mono);
    text-align: left;
    cursor: pointer;
  }

  .ws-entry:hover:not(:disabled) {
    background: var(--accent);
  }

  .ws-entry:disabled {
    cursor: default;
    opacity: 0.55;
  }

  .ws-entry-path {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
    white-space: normal;
  }

  .ws-entry[aria-current="true"] {
    opacity: 1;
  }

  .ws-entry[aria-current="true"] .ws-entry-path {
    color: var(--primary);
  }

  .ws-footer {
    border-top: 1px solid var(--hairline);
    padding-top: 8px;
  }

  .ws-open {
    width: 100%;
  }

  .ws-note {
    margin: 0;
    padding: 4px 8px;
    font-size: 12px;
  }
</style>
