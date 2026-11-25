<script lang="ts">
  import { untrack } from "svelte";
  import { ArrowClockwise } from "phosphor-svelte";
  import BesSetup from "./BesSetup.svelte";
  import CopyButton from "./CopyButton.svelte";
  import Notice from "./Notice.svelte";
  import type { TargetScope, WorkspaceInfo } from "./types";

  let {
    workspace,
    scope,
    info,
    loading,
    error,
    besEndpoint,
    onretry,
    onsavescope,
  }: {
    workspace: string;
    scope: TargetScope;
    info: WorkspaceInfo | null;
    loading: boolean;
    error: string | null;
    besEndpoint: string | null;
    onretry: () => void;
    onsavescope: (scope: TargetScope) => Promise<boolean>;
  } = $props();

  const name = $derived(workspace.split(/[\\/]/).filter(Boolean).at(-1) ?? workspace);
  const persistedScope = $derived(`${workspace}\0${scope.include.join("\n")}`);
  let includeText = $state(untrack(() => scope.include.join("\n")));
  let lastScopeKey = untrack(() => `${workspace}\0${scope.include.join("\n")}`);
  type ScopeSave = { kind: "idle" } | { kind: "saving" } | { kind: "failed"; error: string };
  let save = $state<ScopeSave>({ kind: "idle" });
  const saving = $derived(save.kind === "saving");
  let expanded = $state(false);

  const PRIMARY_KEYS = [
    "workspace",
    "output_base",
    "execution_root",
    "bazel-bin",
    "bazel-testlogs",
    "repository_cache",
  ];
  const entries = $derived(info?.entries ?? []);
  const primary = $derived(
    PRIMARY_KEYS.map((key) => entries.find((e) => e.key === key)).filter(
      (e) => e !== undefined,
    ),
  );
  const rows = $derived(expanded ? entries : primary);

  $effect(() => {
    if (persistedScope !== lastScopeKey) {
      lastScopeKey = persistedScope;
      includeText = scope.include.join("\n");
    }
  });

  async function saveScope(event: SubmitEvent) {
    event.preventDefault();
    if (saving || includeText.trim() === "") return;
    save = { kind: "saving" };
    try {
      await onsavescope({ include: includeText.split("\n") });
      save = { kind: "idle" };
    } catch (e) {
      save = { kind: "failed", error: String(e) };
    }
  }
</script>

<div class="view-head">
  <div>
    <h1 class="view-title mono">{name}</h1>
    <div class="view-meta mono">
      {workspace}{#if info?.version}{" · "}Bazel {info.version}{/if}
    </div>
  </div>
</div>

<section class="section">
  <div class="section-head">
    <h2 class="section-title">Bazel Info</h2>
    <button
      type="button"
      class="button button-ghost icon-button"
      aria-label="Reload Bazel info"
      title="Reload Bazel info"
      disabled={loading}
      onclick={onretry}
    >
      <ArrowClockwise size={14} />
    </button>
  </div>
  {#if error}
    <Notice tone="warning" onaction={onretry}>Could not read bazel info: {error}</Notice>
  {/if}
  {#if loading && !info}
    <p class="dim">Loading bazel info…</p>
  {/if}
  {#if rows.length}
    <table class="lens-table">
      <tbody>
        {#each rows as row (row.key)}
          <tr>
            <td class="mono">{row.key}</td>
            <td class="mono path-cell">
              <div class="workspace-info-value">
                <span>{row.value}</span>
                <CopyButton text={row.value} label={`Copy ${row.key} value`} />
              </div>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
  {#if entries.length > primary.length}
    <button
      type="button"
      class="button button-ghost"
      aria-expanded={expanded}
      onclick={() => (expanded = !expanded)}
    >
      {expanded ? "Show less" : `Show all (${entries.length})`}
    </button>
  {/if}
</section>

<section class="section">
  <div class="section-head">
    <h2 class="section-title">Target Discovery Scope</h2>
  </div>
  <form class="scope-form" onsubmit={saveScope}>
    <textarea
      class="input mono scope-input"
      rows="3"
      aria-label="Target discovery patterns"
      spellcheck={false}
      bind:value={includeText}
      disabled={saving}
    ></textarea>
    {#if save.kind === "failed" && save.error}<Notice tone="error">{save.error}</Notice>{/if}
    <div>
      <button type="submit" class="button" disabled={saving || includeText.trim() === ""}>
        {saving ? "Saving…" : "Save scope"}
      </button>
    </div>
  </form>
</section>

<section class="section">
  <div class="section-head">
    <h2 class="section-title">Local BES Receiver</h2>
  </div>
  <BesSetup endpoint={besEndpoint} />
</section>
