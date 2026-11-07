<script lang="ts">
  import { Play, Stop } from "phosphor-svelte";
  import { tick } from "svelte";
  import SearchInput from "../SearchInput.svelte";
  import type { createIndex } from "../search";
  import type { BazelCommand } from "../types";

  let {
    index,
    value,
    inputError,
    running,
    disabled,
    oninput,
    onpick,
    onstart,
    onstop,
  }: {
    index: ReturnType<typeof createIndex> | null;
    value: string;
    inputError: string | null;
    running: boolean;
    disabled: boolean;
    oninput: (value: string) => void;
    onpick: (value: string) => void;
    onstart: (
      command: BazelCommand,
      target: string,
      argumentsText: string,
    ) => void;
    onstop: () => void;
  } = $props();

  let cmd = $state<BazelCommand>("build");
  let argumentsText = $state("");
  let argumentsOpen = $state(false);
  let argumentsToggle: HTMLButtonElement | undefined;
  let argumentsPanel = $state<HTMLDivElement>();
  let argumentsInput = $state<HTMLInputElement>();
  const argumentsConfigured = $derived(argumentsText.trim() !== "");
  const actionLabel = $derived(
    running ? "Stop" : cmd === "test" ? "Test" : cmd === "run" ? "Run" : "Build",
  );

  function submit() {
    if (running) onstop();
    else onstart(cmd, value, argumentsText);
  }

  async function setArgumentsOpen(open: boolean) {
    argumentsOpen = open;
    await tick();
    if (argumentsOpen === open) {
      (open ? argumentsInput : argumentsToggle)?.focus();
    }
  }

  function argumentsKey(event: KeyboardEvent) {
    if (!argumentsOpen || event.isComposing || event.key !== "Escape") return;
    event.preventDefault();
    void setArgumentsOpen(false);
  }

  function dismissArguments(event: Event) {
    if (
      argumentsOpen &&
      event.target instanceof Node &&
      !argumentsPanel?.contains(event.target) &&
      !argumentsToggle?.contains(event.target)
    ) {
      argumentsOpen = false;
    }
  }
</script>

<svelte:window
  onpointerdown={dismissArguments}
  onfocusin={dismissArguments}
  onkeydown={argumentsKey}
/>

<div class="composer" class:composer-invalid={!!inputError}>
  <select class="select" aria-label="Bazel command" bind:value={cmd}>
    <option>build</option>
    <option>test</option>
    <option>run</option>
  </select>
  <div class="composer-search">
    <SearchInput
      class="input composer-target"
      aria-label="Target label or pattern"
      aria-invalid={!!inputError}
      aria-describedby={inputError ? "composer-target-error" : undefined}
      placeholder="Fuzzy :all / //pkg:target"
      {index}
      {value}
      {oninput}
      {onpick}
    />
  </div>
  <button
    bind:this={argumentsToggle}
    type="button"
    class="button button-ghost arguments-toggle"
    aria-label={argumentsConfigured ? "Arguments (configured)" : "Arguments"}
    aria-expanded={argumentsOpen}
    aria-controls="command-arguments-popover"
    title={argumentsConfigured ? argumentsText : "Specify arguments"}
    onclick={() => setArgumentsOpen(!argumentsOpen)}
  >
    Arguments
    {#if argumentsConfigured}<span class="arguments-indicator" aria-hidden="true"></span>{/if}
  </button>
  {#if argumentsOpen}
    <div
      bind:this={argumentsPanel}
      class="arguments-popover"
      id="command-arguments-popover"
    >
      <input
        bind:this={argumentsInput}
        bind:value={argumentsText}
        id="command-arguments"
        class="input arguments-input"
        aria-label="Arguments"
        placeholder="--config=dev; program arguments after --"
        title="Shell-style quotes and escapes. Quote # to keep it literal. No variable or command expansion."
        autocomplete="off"
        spellcheck={false}
      />
    </div>
  {/if}
  <button
    type="button"
    class="button button-primary icon-button"
    aria-label={actionLabel}
    title={actionLabel}
    {disabled}
    onclick={submit}
  >
    {#if running}<Stop size={12} weight="fill" />{:else}<Play
        size={12}
        weight="fill"
      />{/if}
  </button>
</div>
