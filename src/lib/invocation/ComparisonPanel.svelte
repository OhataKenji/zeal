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
  import type { InvocationDetail, InvocationRecord } from "../types";
  import Notice from "../Notice.svelte";

  let {
    current,
    currentLoading,
    currentError,
    workspace,
    runningIds,
    history = [],
    load,
  }: {
    current: InvocationDetail | null;
    currentLoading: boolean;
    currentError: string | null;
    workspace: string;
    runningIds: readonly string[];
    history?: readonly InvocationRecord[];
    load?: (id: string) => Promise<InvocationDetail>;
  } = $props();
  const fetchDetail = $derived(
    load ?? ((id: string) => invoke<InvocationDetail>(COMMANDS.getInvocation, { workspace, id })),
  );

  type Comparison =
    | { kind: "idle" }
    | { kind: "loading"; id: string }
    | { kind: "failed"; id: string; error: string }
    | { kind: "ready"; id: string; baseline: ComparisonInvocation }
    | { kind: "disposed" };
  let comparison = $state.raw<Comparison>({ kind: "idle" });
  let manualId = $state("");
  onDestroy(() => {
    comparison = { kind: "disposed" };
  });

  const currentActive = $derived(!!current && runningIds.includes(current.record.id));
  const currentReady = $derived(!!current && !currentLoading && !currentError && !currentActive);
  const baseline = $derived(comparison.kind === "ready" ? comparison.baseline : null);
  const baselineActive = $derived(baseline !== null && runningIds.includes(baseline.record.id));
  const baselineReady = $derived(baseline !== null && !baselineActive);
  const previousSuccess = $derived(
    current ? previousSuccessfulInvocation(current.record, history, runningIds) : null,
  );
  const recent = $derived(
    current
      ? recentComparableInvocations(current.record, history, runningIds)
      : { sameTarget: [], other: [] },
  );
  const hasRecent = $derived(recent.sameTarget.length > 0 || recent.other.length > 0);
  const activeBaselineId = $derived("id" in comparison ? comparison.id : "");
  const rows = $derived(
    comparisonRows(baselineReady ? baseline : null, currentReady ? current : null),
  );

  async function loadBaseline(rawId: string) {
    const id = rawId.trim();
    if (!id || !currentReady || comparison.kind === "loading" || comparison.kind === "disposed") {
      return;
    }
    if (runningIds.includes(id)) {
      comparison = {
        kind: "failed",
        id,
        error: "Comparison is available after the invocation finishes.",
      };
      return;
    }
    const request: Comparison = { kind: "loading", id };
    comparison = request;
    try {
      const detail = await fetchDetail(id);
      if (comparison !== request) return;
      comparison = {
        kind: "ready",
        id,
        baseline: { record: detail.record, bep: detail.bep },
      };
    } catch (error) {
      if (comparison !== request) return;
      comparison = { kind: "failed", id, error: String(error) };
    }
  }

  $effect(() => {
    if (!currentReady || !previousSuccess || comparison.kind !== "idle") return;
    void loadBaseline(previousSuccess.id);
  });

  async function submitManual(event: SubmitEvent) {
    event.preventDefault();
    await loadBaseline(manualId);
  }
</script>

{#if currentActive}
  <p class="panel-empty">Comparison is available after the invocation finishes.</p>
{:else}
  <div class="compare-scroll">
    <div class="compare-toolbar">
      <label class="compare-toolbar-label" for="compare-baseline-select">Baseline</label>
      {#if hasRecent}
        <select
          id="compare-baseline-select"
          class="input compare-select"
          disabled={!currentReady || comparison.kind === "loading"}
          value={recent.sameTarget.some((e) => e.id === activeBaselineId) ||
          recent.other.some((e) => e.id === activeBaselineId)
            ? activeBaselineId
            : ""}
          onchange={(event) => {
            const id = event.currentTarget.value;
            if (id) void loadBaseline(id);
          }}
        >
          <option value="">Select a baseline…</option>
          {#if recent.sameTarget.length > 0}
            <optgroup label="Same target">
              {#each recent.sameTarget as rec (rec.id)}
                <option value={rec.id}
                  >{formatInvocationOption(rec, rec.id === previousSuccess?.id)}</option
                >
              {/each}
            </optgroup>
          {/if}
          {#if recent.other.length > 0}
            <optgroup label="Other recent">
              {#each recent.other as rec (rec.id)}
                <option value={rec.id}>{formatInvocationOption(rec)}</option>
              {/each}
            </optgroup>
          {/if}
        </select>
      {/if}
      <details class="compare-by-id">
        <summary>By ID</summary>
        <form class="compare-form" onsubmit={submitManual}>
          <label class="sr-only" for="compare-invocation-id">Comparison invocation</label>
          <input
            id="compare-invocation-id"
            class="input compare-id-input"
            type="text"
            bind:value={manualId}
            disabled={comparison.kind === "loading" || !currentReady}
            autocomplete="off"
            spellcheck="false"
            placeholder="Enter invocation ID and press Enter."
          />
        </form>
      </details>
      {#if comparison.kind === "loading"}
        <span class="compare-status" role="status">Loading…</span>
      {:else if currentReady && !previousSuccess && comparison.kind === "idle"}
        <span class="compare-status">No previous successful build in recent history.</span>
      {/if}
    </div>

    {#if comparison.kind === "failed"}
      <Notice tone="error">{comparison.error}</Notice>
    {/if}
    {#if baselineActive}
      <p class="compare-status">Comparison is available after the invocation finishes.</p>
    {/if}

    <table class="compare-table" aria-label="Invocation comparison">
      <thead>
        <tr>
          <td class="compare-item-head"></td>
          <th scope="col" class="compare-side-head">
            Baseline
            {#if baseline && baseline.bep.status !== "ok"}
              <Notice tone={sectionNoticeTone(baseline.bep)}>{sectionMessage(baseline.bep)}</Notice>
            {/if}
          </th>
          <th scope="col" class="compare-side-head">
            Current
            {#if currentLoading}
              <p class="compare-status" role="status">Loading…</p>
            {:else if currentError}
              <p class="field-error">{currentError}</p>
            {/if}
            {#if current && current.bep.status !== "ok"}
              <Notice tone={sectionNoticeTone(current.bep)}>{sectionMessage(current.bep)}</Notice>
            {/if}
          </th>
          <th scope="col" class="compare-delta-head">Delta</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as compareRow (compareRow.label)}
          <tr class:compare-changed={compareRow.status === "changed"}>
            <th scope="row" class="compare-item">{compareRow.label}</th>
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
            <td class="compare-delta-cell mono">
              {#if compareRow.delta}
                <span
                  class="compare-delta"
                  class:compare-delta-better={compareRow.delta.tone === "better"}
                  class:compare-delta-worse={compareRow.delta.tone === "worse"}
                >{compareRow.delta.text}</span>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
