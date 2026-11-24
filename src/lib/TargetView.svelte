<script lang="ts">
  import { SvelteSet } from "svelte/reactivity";
  import { CaretDown, CaretUp } from "phosphor-svelte";
  import type {
    BazelCommand,
    TargetDetail,
    TargetIndex,
    TargetPhaseData,
    TargetTab,
  } from "./types";
  import {
    displayLabel,
    displayPath,
    isTestRule,
    relations,
    shortOutputPath,
  } from "./model";
  import { tabKeydown } from "./tabs";
  import ActionPair from "./ActionPair.svelte";
  import ActionsSection from "./TargetActions.svelte";
  import CopyButton from "./CopyButton.svelte";

  let {
    index,
    detail,
    phaseData,
    phaseError,
    tab,
    flags,
    wsRoot,
    canRun = true,
    onselect,
    onrun,
    onopenflags,
    ontab,
    onapply,
  }: {
    index: TargetIndex;
    detail: TargetDetail;
    phaseData: TargetPhaseData | null;
    phaseError: string | null;
    tab: TargetTab;
    flags: string;
    wsRoot: string;
    canRun?: boolean;
    onselect: (label: string) => void;
    onrun?: (cmd: BazelCommand, pattern: string) => void;
    onopenflags?: (cmd: BazelCommand, pattern: string) => void;
    ontab: (tab: TargetTab) => void;
    onapply: (flags: string) => void;
  } = $props();
  const rel = $derived(relations(detail));
  const configured = $derived(
    phaseData && "configured" in phaseData ? phaseData.configured : null,
  );
  const actions = $derived(
    phaseData && "actions" in phaseData ? phaseData.actions : null,
  );
  const customFlags = $derived(flags.trim() !== "");
  const tabs = [
    { id: "definition" as const, label: "Definition" },
    { id: "configured" as const, label: "Configured" },
    { id: "actions" as const, label: "Actions" },
  ];
  const target = $derived(rel.target);
  const primary = $derived.by((): { cmd: BazelCommand; label: string } => {
    if (isTestRule(target.ruleClass)) return { cmd: "test", label: "Test" };
    if (target.ruleClass.endsWith("_binary")) return { cmd: "run", label: "Run" };
    return { cmd: "build", label: "Build" };
  });
  const targets = $derived(
    new Map(index.targets.map((target) => [target.label, target])),
  );
  const expanded = new SvelteSet<string>();
  // The flags editor holds a nullable draft: null is the collapsed bar,
  // a string is the in-progress edit. Apply only fires from Enter or the
  // button; nothing runs on draft changes or blur.
  let flagsDraft = $state<string | null>(null);
  function location(file: string, line: number) {
    return `${file.startsWith(wsRoot + "/") ? file.slice(wsRoot.length + 1) : file}:${line}`;
  }
  function applyFlags(event: SubmitEvent) {
    event.preventDefault();
    const flags = flagsDraft?.trim();
    // A blank draft never applies: clearing flags is the explicit ✕ reset.
    if (!flags) return;
    flagsDraft = null;
    onapply(flags);
  }
  function flagKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      flagsDraft = null;
    }
  }
</script>

{#snippet labels(values: string[], id: string, pkg: string)}
  <div class="chips">
    {#each expanded.has(id) ? values : values.slice(0, 12) as value (value)}
      {@const target = targets.get(value)}
      {#if target}
        <button class="chip" title={value} onclick={() => onselect(value)}>
          <span>{displayLabel(value, pkg)}</span><small
            >{target.ruleClass}</small
          >
        </button>
      {:else}<span class="chip" title={value}>{displayLabel(value, pkg)}</span
        >{/if}
    {/each}
    {#if values.length > 12}
      <button
        class="chip more"
        aria-expanded={expanded.has(id)}
        onclick={() =>
          expanded.has(id) ? expanded.delete(id) : expanded.add(id)}
      >
        {#if expanded.has(id)}Show less <CaretUp
            size={10}
          />{:else}+{values.length - 12} more <CaretDown size={10} />{/if}
      </button>
    {/if}
  </div>
{/snippet}

{#snippet files(values: string[], group: string, pkg: string)}
  <div class="chips">
    {#each expanded.has(group) ? values : values.slice(0, 16) as path (path)}
      <span class="chip file" title={path}
        >{displayPath(path, pkg)}</span
      >
    {/each}
    {#if values.length > 16}
      <button
        class="chip more"
        aria-expanded={expanded.has(group)}
        onclick={() =>
          expanded.has(group) ? expanded.delete(group) : expanded.add(group)}
      >
        {expanded.has(group) ? "Show less" : `+${values.length - 16} more`}
        <CaretDown size={10} />
      </button>
    {/if}
    {#if values.length === 0}<span class="dim">None</span>{/if}
  </div>
{/snippet}

{#snippet flagsBar()}
  {#if flagsDraft === null}
    <div class="flag-bar">
      <span class="flag-bar-value mono">
        <span class="dim">Config:</span>
        {#if customFlags}
          <span class="flag-flags" title={flags}>{flags}</span>
        {:else}
          <span class="dim">Default</span>
        {/if}
      </span>
      <span class="flag-bar-actions">
        <button
          type="button"
          class="button button-ghost flag-button"
          onclick={() => (flagsDraft = flags)}
          > {customFlags ? "Edit" : "+ Flags"}</button
        >
        {#if customFlags}
          <button
            type="button"
            class="button button-ghost flag-button"
            aria-label="Reset flags"
            title="Reset to Default"
            onclick={() => onapply("")}>✕</button
          >
        {/if}
      </span>
    </div>
  {:else}
    <form class="flag-bar flag-bar-edit" onsubmit={applyFlags}>
      <input
        class="input mono"
        value={flagsDraft}
        oninput={(event) => (flagsDraft = event.currentTarget.value)}
        onkeydown={flagKeydown}
        placeholder="--config=opt"
        aria-label="Build flags"
        spellcheck="false"
      />
      <button
        type="submit"
        class="button button-primary flag-apply"
        disabled={!flagsDraft.trim()}>Apply</button
      >
      <button
        type="button"
        class="button button-ghost"
        onclick={() => (flagsDraft = null)}>Cancel</button
      >
    </form>
    <p class="flag-warning">
      Changing build flags discards Bazel’s analysis cache for this workspace.
    </p>
  {/if}
{/snippet}

  <header class="target-head">
    <h1>{target.label}</h1>
    <div class="target-head-meta">
      <span class="badge">{target.ruleClass}</span>
      {#if target.location}
        <span class="source-location" title={target.location.file}>
          {location(target.location.file, target.location.line)}
        </span>
      {/if}
    </div>
    {#if onrun && onopenflags}
      <div class="target-head-actions">
        <ActionPair
          primaryCmd={primary.cmd}
          primaryLabel={primary.label}
          target={target.label}
          disabled={!canRun}
          {onrun}
          {onopenflags}
        />
      </div>
    {/if}
  </header>
  <div class="tabs" role="tablist" aria-label="Target views">
    {#each tabs as t}
      <button
        type="button"
        role="tab"
        id="target-tab-{t.id}"
        aria-selected={tab === t.id}
        aria-controls={tab === t.id ? `target-panel-${t.id}` : undefined}
        tabindex={tab === t.id ? 0 : -1}
        onclick={() => ontab(t.id)}
        onkeydown={tabKeydown}>{t.label}{#if t.id === "actions" && actions}<span
            class="dim"
            > ({actions.actions.length}{actions.truncated ? "+" : ""})</span
          >{/if}{#if customFlags && t.id !== "definition"}<span
            class="tab-flag"
            aria-hidden="true">•</span
          >{/if}</button
      >
    {/each}
  </div>
  <div id="target-panel-{tab}" role="tabpanel" aria-labelledby="target-tab-{tab}">
    {#if tab === "definition"}
      <h2 class="attributes-head">Attributes</h2>
      <dl class="relations">
        {#each rel.attributes as attribute (attribute.name)}
          <dt>{attribute.name}</dt>
          <dd>
            {#if attribute.labels.length}
              {@render labels(attribute.labels, `${attribute.name}-labels`, target.pkg)}
            {/if}
            {#if attribute.files.length}
              {@render files(attribute.files, `${attribute.name}-files`, target.pkg)}
            {/if}
            {#if !attribute.labels.length && !attribute.files.length}
              <span class="dim">None</span>
            {/if}
          </dd>
        {/each}
        <dt>outs</dt>
        <dd>
          {@render files(
            [...new Set(rel.outputs.map((file) => file.path))],
            "outs",
            target.pkg,
          )}
        </dd>
      </dl>
    {:else}
      {@render flagsBar()}
      {#if tab === "configured"}
        {#if phaseError}
          <p class="dim">Could not load configuration: {phaseError}</p>
        {:else if configured === null}
          <p class="dim">Loading configuration…</p>
        {:else}
          <section class="configured-section">
            <h2 class="configured-head">Config</h2>
            {#if configured.configuration}
              <div class="configured-config mono">
                <span>{configured.configuration.mnemonic}</span>
                <span class="dim">·</span>
                <span title={configured.configuration.checksum}
                  >{configured.configuration.checksum.slice(0, 12)}</span
                >
                <CopyButton
                  text={configured.configuration.checksum}
                  label={`Copy configuration hash ${configured.configuration.checksum}`}
                />
              </div>
            {:else}
              <p class="dim">No configuration</p>
            {/if}
          </section>
          <section class="configured-section">
            <h2 class="configured-head">
              Resolved Outputs ({configured.outputs.paths.length}{configured.outputs.truncated
                ? "+"
                : ""})
            </h2>
            {#if configured.outputs.paths.length === 0}
              <p class="dim">None</p>
            {:else}
              <div class="configured-rows">
                {#each configured.outputs.paths as path}
                  <div class="configured-row">
                    <span class="configured-path mono" title={path}
                      >{shortOutputPath(path)}</span
                    >
                    <CopyButton text={path} label={`Copy output path ${path}`} />
                  </div>
                {/each}
              </div>
            {/if}
          </section>
          <section class="configured-section">
            <h2 class="configured-head">
              Direct deps &amp; transitions ({configured.deps.length}{configured.depsTruncated
                ? "+"
                : ""})
            </h2>
            {#if configured.deps.length === 0}
              <p class="dim">None</p>
            {:else}
              <div class="configured-rows">
                {#each configured.deps as dep}
                  <div class="configured-row">
                    {#if targets.has(dep.label)}
                      <button
                        type="button"
                        class="link configured-dep mono"
                        title={dep.label}
                        onclick={() => onselect(dep.label)}>{dep.label}</button
                      >
                    {:else}
                      <span class="configured-dep configured-dep-static mono" title={dep.label}
                        >{dep.label}</span
                      >
                    {/if}
                    {#if dep.transition}
                      <span class="badge dep-transition" title={dep.transition}
                        >{dep.transition}</span
                      >
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          </section>
        {/if}
      {:else}
        <ActionsSection actions={actions} error={phaseError} showHeading={false} />
      {/if}
    {/if}
  </div>
