<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onDestroy } from "svelte";
  import { COMMANDS } from "../commands";
  import {
    comparisonRows,
    formatInvocationOption,
    previousSuccessfulInvocation,
    recentComparableInvocations,
    type ComparisonInvocation,
  } from "../invocation-diff";
  import { sectionMessage, sectionNoticeTone } from "../invocation";
  import { createRequest, idleRequest, type RequestState } from "../request";
  import type { InvocationDetail, InvocationRecord } from "../types";
  import Notice from "../Notice.svelte";

  let {
    left,
    leftLoading,
    leftError,
    workspace,
    runningIds,
    history = [],
    load,
  }: {
    left: InvocationDetail | null;
    leftLoading: boolean;
    leftError: string | null;
    workspace: string;
    runningIds: readonly string[];
    history?: readonly InvocationRecord[];
    load?: (id: string) => Promise<InvocationDetail>;
  } = $props();
  const fetchDetail = $derived(
    load ?? ((id: string) => invoke<InvocationDetail>(COMMANDS.getInvocation, { workspace, id })),
  );

  let input = $state("");
  let comparison = $state.raw<RequestState<ComparisonInvocation>>(idleRequest());
  let autoAttempted = false;
  const request = createRequest<ComparisonInvocation>((next) => (comparison = next));
  onDestroy(() => {
    request.dispose();
    comparison = request.state;
  });

  const leftReady = $derived(
    !!left && !leftLoading && !leftError && !runningIds.includes(left.record.id),
  );
  const leftActive = $derived(!!left && runningIds.includes(left.record.id));
  const right = $derived(comparison.kind === "ready" ? comparison.value : null);
  const rightReady = $derived(right !== null && !runningIds.includes(right.record.id));
  const rightActive = $derived(right !== null && runningIds.includes(right.record.id));
  const previousSuccess = $derived(
    left ? previousSuccessfulInvocation(left.record, history, runningIds) : null,
  );
  const recentOptions = $derived(
    left ? recentComparableInvocations(left.record.id, history, runningIds) : [],
  );
  const rows = $derived(comparisonRows(leftReady ? left : null, rightReady ? right : null));
  const summaryRows = $derived(
    rows.filter((candidate) => candidate.label === "Duration" || candidate.label === "Cache hit rate"),
  );
  const selectedRecent = $derived(
    recentOptions.some((entry) => entry.id === input.trim()) ? input.trim() : "",
  );

  async function loadComparison(rawId: string) {
    const id = rawId.trim();
    if (!id || !leftReady || comparison.kind === "loading" || comparison.kind === "disposed") {
      return;
    }
    autoAttempted = true;
    input = id;
    if (runningIds.includes(id)) {
      request.fail("Comparison is available after the invocation finishes.");
      return;
    }
    await request.run(async () => {
      const detail = await fetchDetail(id);
      return { record: detail.record, bep: detail.bep };
    });
  }

  $effect(() => {
    if (autoAttempted || !leftReady || !previousSuccess || comparison.kind !== "idle") {
      return;
    }
    void loadComparison(previousSuccess.id);
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    await loadComparison(input);
  }
</script>

<div class="compare-scroll">
  {#if leftReady && rightReady}
    <div class="compare-summary" aria-label="Comparison summary">
      {#each summaryRows as metric (metric.label)}
        <div class="compare-metric">
          <span class="compare-metric-label">{metric.label}</span>
          <span class="compare-metric-values mono">
            {metric.left} <span class="compare-vs">vs</span> {metric.right}
          </span>
          {#if metric.delta}
            <span
              class="compare-delta mono"
              class:compare-delta-better={metric.delta.tone === "better"}
              class:compare-delta-worse={metric.delta.tone === "worse"}
            >{metric.delta.text}</span>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
  <table class="compare-table" aria-label="Invocation comparison">
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
          <div class="compare-controls">
            <button
              type="button"
              class="button button-primary compare-prev-btn"
              disabled={!leftReady || !previousSuccess || comparison.kind === "loading"}
              onclick={() => previousSuccess && void loadComparison(previousSuccess.id)}
            >
              Compare with previous successful
            </button>
            {#if leftReady && !previousSuccess}
              <p class="compare-status">No previous successful build for this target pattern.</p>
            {/if}
            {#if recentOptions.length > 0}
              <select
                class="input compare-select"
                aria-label="Recent invocations"
                disabled={!leftReady || comparison.kind === "loading"}
                value={selectedRecent}
                onchange={(event) => {
                  const id = event.currentTarget.value;
                  if (id) void loadComparison(id);
                }}
              >
                <option value="">Select a recent invocation…</option>
                {#each recentOptions as rec (rec.id)}
                  <option value={rec.id}>{formatInvocationOption(rec)}</option>
                {/each}
              </select>
            {/if}
            <form class="compare-form" onsubmit={submit}>
              <label for="compare-invocation-id">Comparison invocation</label>
              <input
                id="compare-invocation-id"
                class="input compare-id-input"
                type="text"
                bind:value={input}
                disabled={comparison.kind === "loading" || !leftReady}
                autocomplete="off"
                spellcheck="false"
                placeholder="Enter an invocation ID and press Enter."
              />
            </form>
          </div>
          {#if comparison.kind === "loading"}
            <p class="compare-status" role="status">Loading…</p>
          {:else if comparison.kind === "failed" && comparison.error}
            <Notice tone="error">{comparison.error}</Notice>
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
            <span>{compareRow.label}</span>
            {#if compareRow.delta}
              <span
                class="compare-delta mono"
                class:compare-delta-better={compareRow.delta.tone === "better"}
                class:compare-delta-worse={compareRow.delta.tone === "worse"}
              >{compareRow.delta.text}</span>
            {/if}
          </th>
          <td class="compare-value"
            ><code
              >{#each compareRow.leftParts as part}{#if part.changed}<del class="compare-removed"
                  >{part.text}</del
                >{:else}{part.text}{/if}{/each}</code
            ></td>
          <td class="compare-value"
            ><code
              >{#each compareRow.rightParts as part}{#if part.changed}<ins class="compare-added"
                  >{part.text}</ins
                >{:else}{part.text}{/if}{/each}</code
            ></td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
