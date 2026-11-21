<script lang="ts">
  import { untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { CaretDown, CaretUp } from "phosphor-svelte";
  import type {
    BazelCommand,
    TargetActions,
    TargetDetail,
    TargetIndex,
    TargetOutputs,
  } from "./types";
  import { displayLabel, displayPath, relations } from "./model";
  import { tabKeydown } from "./tabs";
  import ActionPair from "./ActionPair.svelte";
  import ActionsSection from "./TargetActions.svelte";

  let {
    index,
    detail,
    outputs,
    outputsError,
    actions,
    actionsError,
    wsRoot,
    canRun = true,
    onselect,
    onrun,
    onopenflags,
    onclose,
    initialTab = "definition",
  }: {
    index: TargetIndex;
    detail: TargetDetail;
    outputs: TargetOutputs | null;
    outputsError: string | null;
    actions: TargetActions | null;
    actionsError: string | null;
    wsRoot: string;
    canRun?: boolean;
    onselect: (label: string) => void;
    onrun?: (cmd: BazelCommand, pattern: string) => void;
    onopenflags?: (cmd: BazelCommand, pattern: string) => void;
    onclose?: () => void;
    initialTab?: "definition" | "actions";
  } = $props();
  const rel = $derived(relations(detail));
  let tab = $state<"definition" | "actions">(untrack(() => initialTab));
  const actionCount = $derived(
    actions === null ? null : `${actions.actions.length}${actions.truncated ? "+" : ""}`,
  );
  const tabs = $derived([
    { id: "definition" as const, label: "Definition" },
    {
      id: "actions" as const,
      label: `Actions${actionCount === null ? "" : ` (${actionCount})`}`,
    },
  ]);
  function tabKey(event: KeyboardEvent, index: number) {
    tabKeydown(event, index, tabs.length, (next) => (tab = tabs[next].id));
  }
  const target = $derived(rel.target);
  const primary = $derived.by((): { cmd: BazelCommand; label: string } => {
    if (target.ruleClass.endsWith("_test")) return { cmd: "test", label: "Test" };
    if (target.ruleClass.endsWith("_binary")) return { cmd: "run", label: "Run" };
    return { cmd: "build", label: "Build" };
  });
  const targets = $derived(
    new Map(index.targets.map((target) => [target.label, target])),
  );
  const expanded = new SvelteSet<string>();
  function location(file: string, line: number) {
    return `${file.startsWith(wsRoot + "/") ? file.slice(wsRoot.length + 1) : file}:${line}`;
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

  <header class="target-head">
    <div class="target-head-top">
      <h1>{target.label}</h1>
      {#if onclose}
        <button type="button" class="button button-ghost icon-button" aria-label="Close" onclick={onclose}>✕</button>
      {/if}
    </div>
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
    {#each tabs as t, index}
      <button
        type="button"
        role="tab"
        id="target-tab-{t.id}"
        aria-selected={tab === t.id}
        aria-controls={tab === t.id ? `target-panel-${t.id}` : undefined}
        tabindex={tab === t.id ? 0 : -1}
        onclick={() => (tab = t.id)}
        onkeydown={(event) => tabKey(event, index)}>{t.label}</button
      >
    {/each}
  </div>
  {#if tab === "definition"}
    <div id="target-panel-definition" role="tabpanel" aria-labelledby="target-tab-definition">
      <section class="outputs">
        <h2>Outputs</h2>
        {#if outputsError}
          <p class="dim">Could not load outputs: {outputsError}</p>
        {:else if outputs === null}
          <p class="dim">Loading outputs…</p>
        {:else if outputs.paths.length === 0}
          <p class="dim">None</p>
        {:else}
          <div class="chips">
            {#each outputs.paths as path (path)}
              <span class="chip file" title={path}>{path}</span>
            {/each}
            {#if outputs.truncated}<span class="chip more" title="More outputs not shown"
                >…</span
              >{/if}
          </div>
        {/if}
      </section>
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
    </div>
  {:else}
    <div id="target-panel-actions" role="tabpanel" aria-labelledby="target-tab-actions">
      {#key target.label}
        <ActionsSection actions={actions} error={actionsError} />
      {/key}
    </div>
  {/if}
