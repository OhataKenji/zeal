<script lang="ts">
  import { Check, Copy, WarningCircle } from "phosphor-svelte";
  import { onDestroy } from "svelte";
  import { createClipboardFeedback, type ClipboardFeedback } from "./clipboard-feedback";

  let {
    text,
    label,
    disabled = false,
  }: {
    text: string;
    label: string;
    disabled?: boolean;
  } = $props();

  let feedback = $state<ClipboardFeedback | null>(null);
  const clipboard = createClipboardFeedback(
    (value) => navigator.clipboard.writeText(value),
    (result) => (feedback = result),
  );
  onDestroy(clipboard.dispose);
  const result = $derived(feedback?.text === text ? feedback : null);
  const message = $derived(
    !result ? "" : result.ok ? "Copied" : "Copy failed: clipboard unavailable.",
  );
  $effect(() => {
    if (!feedback) return;
    const timer = setTimeout(() => (feedback = null), 1500);
    return () => clearTimeout(timer);
  });
</script>

<button
  type="button"
  class="button button-ghost icon-button"
  aria-label={label}
  title={message || label}
  {disabled}
  onclick={() => clipboard.copy(text)}
>
  {#if !result}
    <Copy size={14} />
  {:else if result.ok}
    <Check size={14} color="var(--success)" />
  {:else}
    <WarningCircle size={14} color="var(--warning)" />
  {/if}
</button>
<span class="sr-only" role="status">{message}</span>
