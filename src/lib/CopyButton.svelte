<script lang="ts">
  import { Check, Copy, WarningCircle } from "phosphor-svelte";

  let {
    text,
    label,
    disabled = false,
  }: {
    text: string;
    label: string;
    disabled?: boolean;
  } = $props();

  let feedback = $state<{ text: string; ok: boolean } | null>(null);
  const result = $derived(feedback?.text === text ? feedback : null);
  const message = $derived(
    !result ? "" : result.ok ? "Copied" : "Copy failed: clipboard unavailable.",
  );
  $effect(() => {
    if (!feedback) return;
    const timer = setTimeout(() => (feedback = null), 1500);
    return () => clearTimeout(timer);
  });

  async function copy() {
    const value = text;
    try {
      await navigator.clipboard.writeText(value);
      feedback = { text: value, ok: true };
    } catch {
      feedback = { text: value, ok: false };
    }
  }
</script>

<button
  type="button"
  class="button button-ghost icon-button"
  aria-label={label}
  title={message || label}
  {disabled}
  onclick={copy}
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
