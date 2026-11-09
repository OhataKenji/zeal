<script lang="ts">
  import { onDestroy } from "svelte";
  import { Check, Copy } from "phosphor-svelte";

  let {
    text,
    label,
    disabled = false,
    onerror,
  }: {
    text: string;
    label: string;
    disabled?: boolean;
    onerror: (message: string) => void;
  } = $props();

  // The icon flips to a check for a moment, like GitHub code blocks.
  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;

  onDestroy(() => {
    disposed = true;
    clearTimeout(timer);
  });

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      if (disposed) return;
      copied = true;
      clearTimeout(timer);
      timer = setTimeout(() => (copied = false), 1500);
    } catch {
      if (!disposed)
        onerror(
          "Clipboard access is unavailable — copy the displayed text manually.",
        );
    }
  }
</script>

<button
  type="button"
  class="copy-button"
  aria-label={label}
  title={label}
  {disabled}
  onclick={copy}
>
  {#if copied}<Check size={14} />{:else}<Copy size={14} />{/if}
</button>
