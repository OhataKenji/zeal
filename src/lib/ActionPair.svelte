<script lang="ts">
  import type { BazelCommand } from "./types";

  let {
    primaryCmd,
    primaryLabel,
    target,
    disabled = false,
    onrun,
    onopenflags,
  }: {
    primaryCmd: BazelCommand;
    primaryLabel: string;
    target: string;
    disabled?: boolean;
    onrun: (cmd: BazelCommand, pattern: string) => void;
    onopenflags: (cmd: BazelCommand, pattern: string) => void;
  } = $props();

  let menu = $state<HTMLDetailsElement>();

  function close() {
    if (menu) menu.open = false;
  }

  function onWindowPointerDown(event: PointerEvent) {
    if (menu?.open && !menu.contains(event.target as Node)) close();
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="action-pair">
  <button
    type="button"
    class="button button-primary"
    {disabled}
    onclick={() => onrun(primaryCmd, target)}
  >
    {primaryLabel}
  </button>
  <details class="dd" bind:this={menu}>
    <summary class="button button-ghost" aria-label="More actions">⋯</summary>
    <div class="dd-menu dd-menu-right dd-menu-compact">
      {#if primaryCmd !== "build"}
        <button
          type="button"
          class="menu-item"
          {disabled}
          onclick={() => {
            close();
            onrun("build", target);
          }}
        >
          Build
        </button>
      {/if}
      <button
        type="button"
        class="menu-item"
        {disabled}
        onclick={() => {
          close();
          onopenflags(primaryCmd, target);
        }}
      >
        <span>Run with flags…</span>
        <span class="menu-meta">⌘⇧R</span>
      </button>
    </div>
  </details>
</div>
