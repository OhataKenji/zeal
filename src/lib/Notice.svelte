<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    tone,
    id,
    announce = true,
    onaction,
    actionLabel = "Retry",
    children,
  }: {
    tone: "error" | "warning" | "neutral";
    id?: string;
    announce?: boolean;
    onaction?: () => void;
    actionLabel?: string;
    children: Snippet;
  } = $props();
</script>

<div
  {id}
  class="notice notice--{tone}"
  role={announce ? (tone === "error" ? "alert" : tone === "warning" ? "status" : undefined) : undefined}
>
  <div class="notice-body">{@render children()}</div>
  {#if onaction}<button type="button" class="button" onclick={onaction}>{actionLabel}</button>{/if}
</div>
