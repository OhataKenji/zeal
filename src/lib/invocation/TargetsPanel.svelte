<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onDestroy } from "svelte";
  import { COMMANDS } from "../commands";
  import type { Bep, Section, TargetIndex, TestLogQuery, TestSummary } from "../types";
  import { sectionData, sectionMessage, sectionNoticeTone } from "../invocation";
  import LogPanel from "./LogPanel.svelte";
  import Notice from "../Notice.svelte";

  const AUTO_OPEN_STATUSES = new Set(["FAILED", "TIMEOUT", "REMOTE_FAILURE", "FLAKY"]);

  type TestLog =
    | { kind: "idle" }
    | { kind: "closed" }
    | { kind: "disposed" }
    | { kind: "loading"; key: string; query: TestLogQuery }
    | { kind: "ready"; key: string; query: TestLogQuery; section: Section<string> }
    | { kind: "failed"; key: string; query: TestLogQuery; error: string };

  let {
    section,
    index = null,
    workspace = "",
    invocationId = "",
    onselecttarget,
  }: {
    section: Section<Bep> | undefined;
    index?: TargetIndex | null;
    workspace?: string;
    invocationId?: string;
    onselecttarget?: (label: string) => void;
  } = $props();
  const bep = $derived(sectionData(section));
  const tests = $derived(bep?.tests ?? []);
  const labels = $derived(new Set((index?.targets ?? []).map((t) => t.label)));
  const canReadLog = $derived(Boolean(workspace && invocationId));

  let testLog = $state.raw<TestLog>({ kind: "idle" });
  onDestroy(() => {
    testLog = { kind: "disposed" };
  });

  $effect(() => {
    if (testLog.kind !== "idle" || !canReadLog) return;
    const failed = tests.filter((t) => AUTO_OPEN_STATUSES.has(t.status) && t.hasLog);
    if (!failed.length) return;
    if (failed.length <= 3) openLog(failed[0]);
    else testLog = { kind: "closed" };
  });

  const targetKey = (test: TestSummary) => `${test.label}\0${test.configuration ?? ""}`;

  function openLog(test: TestSummary) {
    const key = targetKey(test);
    const query: TestLogQuery = test.configuration
      ? { label: test.label, configuration: test.configuration }
      : { label: test.label };
    loadLog(key, query);
  }

  function loadLog(key: string, query: TestLogQuery) {
    const request: TestLog = { kind: "loading", key, query };
    testLog = request;
    void invoke<Section<string>>(COMMANDS.getTestLog, {
      workspace,
      id: invocationId,
      query,
    }).then(
      (result) => {
        if (testLog === request) testLog = { kind: "ready", key, query, section: result };
      },
      (err) => {
        if (testLog === request) {
          testLog = {
            kind: "failed",
            key,
            query,
            error: err instanceof Error ? err.message : String(err),
          };
        }
      },
    );
  }

  function toggle(test: TestSummary) {
    if ("key" in testLog && testLog.key === targetKey(test)) {
      testLog = { kind: "closed" };
      return;
    }
    openLog(test);
  }

  function statusTone(status: string): "pass" | "warn" | "fail" {
    if (status === "PASSED") return "pass";
    return status === "FLAKY" ? "warn" : "fail";
  }
</script>

{#snippet targetLabel(label: string)}
  {@const key = label.startsWith("@@//") ? label.slice(2) : label}
  {#if onselecttarget && labels.has(key)}
    <button type="button" class="table-link mono" onclick={() => onselecttarget?.(key)}
      >{label}</button
    >
  {:else}
    <code>{label}</code>
  {/if}
{/snippet}

{#snippet testsSection()}
  <h3 class="panel-subheading">Tests {#if bep}<span>{tests.length}</span>{/if}</h3>
  {#if bep}
    {#if tests.length}
      <ul class="result-list test-result-list">
        {#each tests as test}
          {@const key = targetKey(test)}
          {@const tone = statusTone(test.status)}
          {@const isOpen = "key" in testLog && testLog.key === key}
          <li>
            <div class="test-row">
              <span class:success={tone === "pass"} class:failed={tone === "fail"} class:warning={tone === "warn"}
                >{tone === "pass" ? "✓" : tone === "warn" ? "!" : "✗"}</span
              >
              <div class="result-identity">
                {@render targetLabel(test.label)}
                {#if test.configuration}<div class="result-context">Configuration: <code>{test.configuration}</code></div>{/if}
              </div>
              {#if test.hasLog && canReadLog}
                <button
                  type="button"
                  class="button button-ghost test-log-toggle"
                  aria-expanded={isOpen}
                  onclick={() => toggle(test)}
                >{isOpen ? "Hide log" : "View log"}</button>
              {/if}
              <span class="test-status test-status-{tone}">{test.status}</span>
            </div>
            {#if isOpen && "key" in testLog}
              {@const active = testLog}
              <div class="test-log-drawer" role="region" aria-label="Test log for {test.label}">
                {#if active.kind === "loading"}
                  <p class="panel-empty" role="status">Loading test log…</p>
                {:else if active.kind === "failed"}
                  <Notice tone="error" onaction={() => loadLog(active.key, active.query)}
                    >Could not load test log: {active.error}</Notice
                  >
                {:else if active.kind === "ready"}
                  <LogPanel
                    section={active.section}
                    loading={false}
                    ariaLabel="Test log for {test.label}"
                    onretry={() => loadLog(active.key, active.query)}
                  />
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="panel-empty">No results recorded.</p>
    {/if}
  {/if}
{/snippet}

{#snippet buildResultsSection()}
  <h3 class="panel-subheading">Build results {#if bep}<span>{bep.targets.length}</span>{/if}</h3>
  {#if bep}
    {#if bep.targets.length}
      <ul class="result-list">
        {#each bep.targets as target}
          <li>
            <span class:success={target.ok} class:failed={!target.ok}>{target.ok ? "✓" : "✗"}</span>
            <div class="result-identity">
              {@render targetLabel(target.label)}
              {#if target.aspect}<div class="result-context">Aspect: <code>{target.aspect}</code></div>{/if}
              {#if target.configuration}<div class="result-context">Configuration: <code>{target.configuration}</code></div>{/if}
            </div>
            <span>{target.ok ? "Completed" : "Failed"}</span>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="panel-empty">No results recorded.</p>
    {/if}
  {/if}
{/snippet}

{#if section && section.status !== "ok"}<Notice tone={sectionNoticeTone(section)}
    >{sectionMessage(section)}</Notice
  >{/if}

{#if tests.length > 0}
  {@render testsSection()}
  {@render buildResultsSection()}
{:else}
  {@render buildResultsSection()}
  {@render testsSection()}
{/if}
