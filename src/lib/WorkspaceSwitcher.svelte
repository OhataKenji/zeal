<script lang="ts">
  import { onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { CaretDown, Check, Gear } from "phosphor-svelte";
  import { COMMANDS } from "./commands";
  import Notice from "./Notice.svelte";

  let {
    workspace,
    settingsOpen,
    opening,
    onopen,
    onsettings,
    onhome,
  }: {
    workspace: string | null;
    settingsOpen: boolean;
    opening: boolean;
    onopen: (path?: string) => void;
    onsettings: () => void;
    onhome: () => void;
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
    onopen(path);
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

<div class="workspace-switcher">
  <button
    type="button"
    class="ws-home"
    aria-label="Workspace home"
    title="Workspace home"
    disabled={!workspace || opening}
    onclick={() => navigate(onhome)}
  >
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="76 76 360 360"
      width="22"
      height="22"
      class="zeal-logo"
      aria-hidden="true"
    >
      <polygon points="84,104 428,104 428,216 276,216 164,304 84,304" fill="#34d399" />
      <polygon points="428,208 348,208 236,296 84,296 84,408 428,408" fill="#0891b2" />
    </svg>
    <span class="ws-name">{displayName}</span>
  </button>
  <button
    type="button"
    class="button button-ghost icon-button"
    aria-label="Workspace settings"
    title="Workspace settings"
    aria-pressed={settingsOpen}
    disabled={!workspace || opening}
    onclick={() => navigate(onsettings)}
  >
    <Gear size={14} />
  </button>
  <div class="ws-path-row">
    {#if workspace}
      <span class="ws-path" title={workspace}>{workspace}</span>
    {/if}
    <details bind:this={details} class="ws-details" ontoggle={onToggle}>
      <summary
        bind:this={summary}
        class="ws-summary"
        aria-label="Switch workspace"
        title="Switch workspace"
      >
        <CaretDown size={12} />
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
            onclick={() => navigate(onopen)}
            disabled={opening}
          >
            Open workspace…
          </button>
        </div>
      </div>
    </details>
  </div>
</div>

<style>
  .workspace-switcher {
    position: relative;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    column-gap: 8px;
    row-gap: 0;
    min-width: 0;
    padding: 0 12px;
  }

  .workspace-switcher > button[aria-pressed="true"] {
    background: var(--accent);
    color: var(--primary);
  }

  .workspace-switcher > .icon-button {
    width: 24px;
    height: 24px;
  }

  .ws-path-row {
    flex: 0 0 100%;
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
  }

  .ws-home {
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: calc(100% - 32px);
    min-width: 0;
    border: 0;
    padding: 0;
    background: none;
    color: var(--foreground);
    font: 650 13px var(--font-sans);
    text-align: left;
  }

  .zeal-logo {
    flex-shrink: 0;
  }

  .ws-home:hover:not(:disabled) {
    color: var(--primary);
  }

  .ws-details {
    flex: 0 0 auto;
  }

  .ws-summary {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    cursor: pointer;
    list-style: none;
  }

  .ws-summary::-webkit-details-marker {
    display: none;
  }

  .ws-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ws-path {
    display: block;
    min-width: 0;
    max-width: 100%;
    overflow: hidden;
    color: var(--muted-foreground);
    font: 12px var(--mono);
    text-overflow: ellipsis;
    white-space: nowrap;
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
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--background);
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
    border-top: 1px solid var(--border);
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
