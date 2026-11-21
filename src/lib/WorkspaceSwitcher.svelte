<script lang="ts">
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
  let entries = $state.raw<Promise<string[]> | null>(null);

  const displayName = $derived(
    workspace?.split(/[\\/]/).filter(Boolean).at(-1) ?? "Zeal",
  );

  function load() {
    entries = invoke<string[]>(COMMANDS.listWorkspaces);
  }

  function onToggle() {
    if (details?.open) load();
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

  function navigate(action: () => void) {
    if (details) details.open = false;
    action();
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} onkeydown={onKeydown} />

<details bind:this={details} class="dd ws-switch" ontoggle={onToggle}>
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
  <div class="dd-menu ws-menu">
    {#if entries}
      {#await entries}
        <p class="dim ws-note">Loading workspaces…</p>
      {:then paths}
        {#each paths as path (path)}
          <button
            type="button"
            class="menu-item ws-entry"
            disabled={opening || path === workspace}
            aria-current={path === workspace ? "true" : undefined}
            title={path}
            onclick={() => navigate(() => onswitch(path))}
          >
            <span class="ws-entry-path">{path}</span>
            {#if path === workspace}<Check size={12} />{/if}
          </button>
        {/each}
        {#if paths.length === 0}
          <p class="dim ws-note">No recent workspaces.</p>
        {/if}
      {:catch error}
        <Notice tone="error" onaction={load}>{String(error)}</Notice>
      {/await}
    {/if}
    <div class="ws-footer">
      <button
        type="button"
        class="button button-ghost ws-open"
        onclick={() => navigate(onopennew)}
        disabled={opening}
      >
        Open workspace…
      </button>
    </div>
  </div>
</details>

<style>
  .ws-switch {
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
    text-align: left;
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
    width: min(420px, calc(100vw - 24px));
    max-height: min(70vh, 520px);
    overflow: auto;
  }

  .ws-menu .ws-entry {
    font: 12px var(--mono);
    white-space: normal;
  }

  .ws-entry:disabled {
    opacity: 0.55;
  }

  .ws-entry-path {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .ws-entry[aria-current="true"] .ws-entry-path {
    color: var(--primary);
  }

  .ws-footer {
    margin-top: 4px;
    border-top: 1px solid var(--hairline);
    padding-top: 6px;
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
