<script lang="ts">
  import { Check, X, Circle, ArrowClockwise, Stop } from "phosphor-svelte";
  import type { RunStatus } from "./run";

  let {
    kind,
    label,
    compact = false,
  }: RunStatus & { compact?: boolean } = $props();
  const size = $derived(compact ? 12 : 24);
</script>

<span
  class="run-status-icon"
  class:compact
  class:success={kind === "succeeded"}
  class:failed={kind === "failed"}
  role="img"
  aria-label={label}
  title={label}
>
  {#if kind === "succeeded"}<Check {size} />
  {:else if kind === "failed"}<X {size} />
  {:else if kind === "running"}<ArrowClockwise {size} />
  {:else if kind === "stopping"}<Stop {size} weight="fill" />
  {:else}<Circle {size} />{/if}
</span>

<style>
  .run-status-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 40px;
    height: 40px;
    border-radius: 12px;
    color: var(--note);
    background: var(--surface);
  }
  .success {
    color: var(--green);
    background: #e8f4e9;
  }
  .failed {
    color: var(--err);
    background: #fff0ee;
  }
  .compact {
    width: 12px;
    height: 12px;
    align-self: center;
    background: transparent;
  }
</style>
