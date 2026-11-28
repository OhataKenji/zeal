<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onDestroy } from "svelte";
  import { COMMANDS } from "../commands";
  import {
    comparisonRows,
    filterBaselineOptions,
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
  let query = $state("");
  let open = $state(false);
  let activeIndex = $state(0);
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
  const options = $derived(filterBaselineOptions(query, recent, previousSuccess?.id));
  const clampedIndex = $derived(
    options.length > 0 ? Math.min(activeIndex, options.length - 1) : -1,
  );
  const activeBaselineLabel = $derived(
    baseline
      ? formatInvocationOption(baseline.record, baseline.record.id === previousSuccess?.id)
      : "id" in comparison
        ? comparison.id
        : "",
  );
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

  function pickBaseline(id: string) {
    open = false;
    query = "";
    activeIndex = 0;
    void loadBaseline(id);
  }

  function stepActive(delta: number) {
    if (options.length === 0) return;
    open = true;
    activeIndex = (clampedIndex + delta + options.length) % options.length;
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      open = false;
      query = "";
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      stepActive(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const target = (open ? options[clampedIndex]?.id : undefined) || query.trim();
      if (target) pickBaseline(target);
    }
  }

  $effect(() => {
    if (!currentReady || !previousSuccess || comparison.kind !== "idle") return;
    void loadBaseline(previousSuccess.id);
  });
</script>

{#if currentActive}
  <p class="panel-empty">Comparison is available after the invocation finishes.</p>
{:else}
  <div class="compare-scroll">
    <div class="compare-toolbar">
      <label class="compare-toolbar-label" for="compare-baseline-input">Baseline</label>
      <div class="compare-picker">
        <input
          id="compare-baseline-input"
          type="text"
          role="combobox"
          class="input compare-select"
          autocomplete="off"
          spellcheck="false"
          aria-expanded={open}
          aria-controls="compare-baseline-listbox"
          aria-autocomplete="list"
          placeholder="Search baseline or paste invocation ID…"
          disabled={!currentReady || comparison.kind === "loading"}
          value={open ? query : activeBaselineLabel}
          onfocus={() => {
            open = true;
            query = "";
            activeIndex = 0;
          }}
          onblur={() => {
            open = false;
            query = "";
          }}
          oninput={(event) => {
            open = true;
            query = event.currentTarget.value;
            activeIndex = 0;
          }}
          onkeydown={handleKeydown}
        />
        {#if open && options.length > 0}
          <ul id="compare-baseline-listbox" class="compare-listbox" role="listbox">
            {#each options as option, index (`${option.group}:${option.id}`)}
              {#if index === 0 || options[index - 1]?.group !== option.group}
                <li class="compare-listbox-group" role="presentation">{option.group}</li>
              {/if}
              <li
                role="option"
                class="compare-listbox-option"
                class:is-active={index === clampedIndex}
                aria-selected={index === clampedIndex}
                onmousedown={(event) => {
                  event.preventDefault();
                  pickBaseline(option.id);
                }}
              >
                {option.label}
              </li>
            {/each}
          </ul>
        {/if}
      </div>
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
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
