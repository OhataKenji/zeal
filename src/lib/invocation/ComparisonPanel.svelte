<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onDestroy } from "svelte";
  import { COMMANDS } from "../commands";
  import { comparisonRows, type ComparisonInvocation } from "../invocation-diff";
  import { sectionMessage, sectionNoticeTone } from "../invocation";
  import type { InvocationDetail } from "../types";
  import Notice from "../Notice.svelte";

  let {
    left,
    leftLoading,
    leftError,
    activeInvocation,
    load = (id: string) => invoke<InvocationDetail>(COMMANDS.getInvocation, { id }),
  }: {
    left: InvocationDetail | null;
    leftLoading: boolean;
    leftError: string | null;
    activeInvocation: string | null;
    load?: (id: string) => Promise<InvocationDetail>;
  } = $props();

  let input = $state("");
  let right = $state.raw<ComparisonInvocation | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let disposed = false;
  onDestroy(() => {
    disposed = true;
  });

  const leftReady = $derived(
    !!left && !leftLoading && !leftError && left.record.id !== activeInvocation,
  );
  const leftActive = $derived(!!left && left.record.id === activeInvocation);
  const rightReady = $derived(right !== null && right.record.id !== activeInvocation);
  const rightActive = $derived(right !== null && right.record.id === activeInvocation);
  const rows = $derived(comparisonRows(leftReady ? left : null, rightReady ? right : null));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const id = input.trim();
    if (!id || loading || !leftReady) return;
    right = null;
    error = null;
    if (id === activeInvocation) {
      error = "Comparison is available after the invocation finishes.";
      return;
    }
    loading = true;
    try {
      const detail = await load(id);
      if (!disposed) right = { record: detail.record, bep: detail.bep };
    } catch (cause) {
      if (!disposed) error = String(cause);
    } finally {
      if (!disposed) loading = false;
    }
  }
</script>

<div class="compare-scroll">
  <table class="compare-table">
    <caption>Invocation comparison</caption>
    <thead>
      <tr>
        <td class="compare-item-head"></td>
        <th scope="col" class="compare-side-head">
          Current invocation
          {#if leftLoading}
            <p class="compare-status" role="status">Loading…</p>
          {:else if leftError}
            <p class="field-error">{leftError}</p>
          {:else if leftActive}
            <p class="compare-status">
              Comparison is available after the invocation finishes.
            </p>
          {/if}
          {#if left && left.bep.status !== "ok"}
            <Notice tone={sectionNoticeTone(left.bep)}>{sectionMessage(left.bep)}</Notice>
          {/if}
        </th>
        <th scope="col" class="compare-side-head">
          <form class="compare-form" onsubmit={submit}>
            <label class="compare-label" for="compare-invocation-id">Comparison invocation</label>
            <input
              id="compare-invocation-id"
              class="input compare-id-input"
              type="text"
              bind:value={input}
              disabled={loading || !leftReady}
              autocomplete="off"
              spellcheck="false"
              placeholder="Enter an invocation ID and press Enter."
            />
          </form>
          {#if loading}
            <p class="compare-status" role="status">Loading…</p>
          {/if}
          {#if error}
            <Notice tone="error">{error}</Notice>
          {/if}
          {#if rightActive}
            <p class="compare-status">
              Comparison is available after the invocation finishes.
            </p>
          {/if}
          {#if right && right.bep.status !== "ok"}
            <Notice tone={sectionNoticeTone(right.bep)}>{sectionMessage(right.bep)}</Notice>
          {/if}
        </th>
      </tr>
    </thead>
    <tbody>
      {#each rows as compareRow (compareRow.label)}
        <tr class:compare-changed={compareRow.status === "changed"}>
          <th scope="row" class="compare-item">
            {compareRow.label}{#if compareRow.status === "changed"}<span
                class="compare-change">Changed</span
              >{/if}
          </th>
          <td class="compare-value"><code>{compareRow.left}</code></td>
          <td class="compare-value"><code>{compareRow.right}</code></td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
