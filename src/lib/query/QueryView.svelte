<script lang="ts">
  import Notice from "../Notice.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { Bookmarks, CaretDown, Check, Copy } from "phosphor-svelte";
  import { onDestroy, untrack } from "svelte";
  import { createIndex } from "../search";
  import QueryExpr from "./QueryExpr.svelte";
  import QueryGraph from "./QueryGraph.svelte";
  import {
    applyExample,
    attrCandidates,
    codeDiverged,
    defaultNodeFor,
    EXAMPLES,
    kindCandidates,
    parse,
    serialize,
    validate,
    type Expr,
  } from "./language";
  import {
    commandLine,
    createQuery,
    DEFAULT_FLAGS,
    emptyQueryState,
    type CommandFlags,
    type QueryState,
  } from "./execution";
  import type { TargetIndex } from "../types";

  let {
    snap,
    busyReason,
  }: { snap: TargetIndex | null; busyReason: string | null } = $props();

  const searchIndex = $derived(
    snap ? createIndex(snap.targets) : null,
  );
  // Attribute names come from a static hint list: the label_kind snapshot
  // carries no attributes, and completion only assists.
  const attrIndex = createIndex(attrCandidates());
  // kind()'s pattern arg searches a wildcard-test hint, the non-rule kinds,
  // and every rule class in the snapshot.
  const kindIndex = $derived(
    snap ? createIndex(kindCandidates(snap.targets)) : null,
  );
  let root = $state<Expr>(
    EXAMPLES[0]?.build() ?? defaultNodeFor("target"),
  );
  let mode = $state<"builder" | "code">("builder");
  let draft = $state("");
  let codeError = $state<string | null>(null);
  let noImplicit = $state(DEFAULT_FLAGS.noImplicit);
  let noTool = $state(DEFAULT_FLAGS.noTool);
  let resultView = $state<"raw" | "graph">("raw");
  let feedback = $state("");
  let fbTimer: ReturnType<typeof setTimeout> | undefined;
  let qstate = $state<QueryState>(emptyQueryState());
  let controller = $state<ReturnType<typeof createQuery> | null>(null);
  const EXAMPLE_GROUPS = [
    { title: "Dependencies", ids: ["deps", "rdeps", "siblings", "buildfiles"] },
    { title: "Paths", ids: ["somepath", "allpaths"] },
    { title: "Filter & Attributes", ids: ["kindConstraint", "attrLabels"] },
    { title: "Set Operations", ids: ["osExceptKind", "depsIntersectKind"] },
  ] as const;

  let viewEl = $state<HTMLElement>();
  let detailsEl = $state<HTMLDetailsElement>();

  $effect(() => {
    const next = createQuery({ invoke }, (s) => (qstate = s));
    controller = next;
    return next.dispose;
  });
  onDestroy(() => {
    clearTimeout(fbTimer);
    clearTimeout(copyTimer);
  });

  const flags = $derived<CommandFlags>({ noImplicit, noTool });
  const serialized = $derived(serialize(root));
  const builderOk = $derived(serialized.ok);
  const builderText = $derived(serialized.ok ? serialized.text : "");
  const exprText = $derived(mode === "code" ? draft : builderText);
  const errors = $derived(
    mode === "builder" ? validate(root) : new Map<string, string>(),
  );
  const command = $derived(
    mode === "builder" && !serialized.ok
      ? `(serialize error) ${serialized.error}`
      : commandLine(exprText, resultView, flags),
  );
  const valid = $derived(
    mode === "builder" ? builderOk && errors.size === 0 : codeValid(draft),
  );
  const diverged = $derived(mode === "code" && codeDiverged(root, draft));
  const codeWarning = $derived(
    mode === "code" && (diverged || codeError !== null),
  );

  $effect.pre(() => {
    const expr = exprText;
    const view = resultView;
    const options = flags;
    untrack(() => controller?.select(expr, view, options));
  });

  const canRun = $derived(valid && !busyReason && !qstate.running);
  const runBlockReason = $derived(
    qstate.running
      ? null
      : (busyReason ??
        (!valid
          ? mode === "builder"
            ? "fix the marked fields"
            : "enter a query expression"
          : null)),
  );

  // Input changes restore a cached result, or leave the result empty.
  const stored = $derived(qstate.result);
  // Only one invocation is ever in flight; which view started it does not matter —
  // either pane may as well show the spinner.
  const runningThisView = $derived(qstate.running);
  const failedOutput = $derived.by(() => {
    if (!stored) return null;
    if (stored.timedOut)
      return { summary: "Query timed out.", tail: stored.stderrTail };
    if (stored.exitCode !== 0)
      return {
        summary: `Query failed${stored.exitCode === null ? "" : ` (exit ${stored.exitCode})`}.`,
        tail: stored.stderrTail,
      };
    return null;
  });

  const RAW_LINE_CAP = 5000;
  const rawLines = $derived.by(() => {
    if (resultView !== "raw" || !stored) return [] as string[];
    const lines = stored.stdout.split("\n");
    if (lines.at(-1) === "") lines.pop();
    return lines;
  });
  const rawShown = $derived(rawLines.slice(0, RAW_LINE_CAP));
  const rawMore = $derived(rawLines.length - rawShown.length);
  const resultCount = $derived(
    resultView === "raw" && stored && !failedOutput && !runningThisView
      ? `${rawLines.length} target${rawLines.length === 1 ? "" : "s"}`
      : "",
  );
  // A successful graph invocation can still drop nodes (`--graph:node_limit`); bazel
  // only says so on stderr.
  const graphWarn = $derived.by(() => {
    if (resultView !== "graph" || !stored || failedOutput || qstate.running)
      return null;
    const lines = stored.stderrTail
      .split("\n")
      .filter((l) => l.startsWith("WARNING"));
    return lines.length ? lines.join("\n") : null;
  });

  const graphDot = $derived(
    resultView === "graph" &&
      stored &&
      !failedOutput &&
      !stored.truncated &&
      !qstate.running
      ? stored.stdout
      : null,
  );

  function setMode(next: "builder" | "code") {
    if (next === mode) return;
    if (next === "code") {
      if (!serialized.ok) {
        setFeedback(
          `Cannot serialize the Builder expression: ${serialized.error}`,
        );
        return;
      }
      draft = serialized.text;
      codeError = null;
      mode = "code";
      return;
    }
    if (!codeDiverged(root, draft)) {
      codeError = null;
      mode = "builder";
      return;
    }
    const parsed = parse(draft);
    if (!parsed.ok) {
      codeError = parsed.error;
      return;
    }
    const errs = validate(parsed.expr);
    if (errs.size) {
      codeError = errs.values().next().value ?? "Invalid expression";
      return;
    }
    root = parsed.expr;
    codeError = null;
    mode = "builder";
  }

  function run() {
    if (!canRun) return;
    void controller?.run(exprText, resultView, flags);
  }

  // Mirrors the core's `validate_expr` so Execute is never enabled for an
  // expression the command would reject. Tab/LF/CR stay allowed.
  // oxlint-disable-next-line no-control-regex -- control characters are the thing being rejected
  const CTRL_REJECT = /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/;
  function codeValid(draft: string): boolean {
    return (
      draft.trim() !== "" &&
      !draft.trimStart().startsWith("-") &&
      !CTRL_REJECT.test(draft) &&
      new TextEncoder().encode(draft).length <= 100_000
    );
  }

  // Loading an example only fills the editor; the user decides when to run.
  function loadExample(id: string) {
    const built = applyExample(id);
    if (!built) return;
    root = built;
    mode = "builder";
    draft = "";
    codeError = null;
    noImplicit = DEFAULT_FLAGS.noImplicit;
    noTool = DEFAULT_FLAGS.noTool;
    if (detailsEl) detailsEl.open = false;
  }

  function revertToBuilder() {
    draft = builderText;
    codeError = null;
    mode = "builder";
  }

  function setFeedback(msg: string) {
    feedback = msg;
    clearTimeout(fbTimer);
    fbTimer = setTimeout(() => (feedback = ""), 3000);
  }

  // The icon flips to a check for a moment, like GitHub code blocks.
  let copied = $state<"command" | "output" | null>(null);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;
  async function copyToClipboard(text: string, which: "command" | "output") {
    try {
      await navigator.clipboard.writeText(text);
      copied = which;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = null), 1500);
    } catch {
      setFeedback(
        "Clipboard access is unavailable — copy the displayed text manually.",
      );
    }
  }

  function onKey(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      if (viewEl?.contains(e.target as Node) && canRun) {
        e.preventDefault();
        run();
      }
    }
  }

</script>

<svelte:window onkeydown={onKey} />

<section class="query-view" bind:this={viewEl}>
  <div class="query-main">
    <div class="query-content">
      <header class="query-page-header">
        <div class="query-title-row"><h1>Query editor</h1></div>
      </header>
      <section class="query-panel" aria-labelledby="query-heading">
        <div class="query-panel-head">
          <div class="query-panel-title-group">
            <h2 id="query-heading">Bazel query</h2>
            <details
              class="query-examples-wrap"
              bind:this={detailsEl}
              open={typeof location !== "undefined" &&
                new URLSearchParams(location.search).has("examples")}
            >
              <summary class="query-examples-trigger">
                <Bookmarks size={14} />
                <span>Examples</span>
                <span class="query-caret">
                  <CaretDown size={12} />
                </span>
              </summary>
              <button
                type="button"
                class="query-examples-backdrop"
                tabindex="-1"
                aria-label="Close examples menu"
                onclick={() => {
                  if (detailsEl) detailsEl.open = false;
                }}
              ></button>
              <div class="query-examples-dropdown" role="menu" aria-label="Query examples">
                {#each EXAMPLE_GROUPS as group}
                  <div class="query-examples-group">
                    <div class="query-examples-group-title">{group.title}</div>
                    {#each group.ids as itemId}
                      {@const example = EXAMPLES.find((e) => e.id === itemId)}
                      {#if example}
                        <button
                          type="button"
                          class="query-example-item"
                          role="menuitem"
                          onclick={() => loadExample(example.id)}
                        >
                          <code>{example.name}</code>
                          <span class="query-ex-desc">{example.desc}</span>
                        </button>
                      {/if}
                    {/each}
                  </div>
                {/each}
              </div>
            </details>
          </div>
          <div class="query-tabs" role="tablist" aria-label="Query mode">
            <button
              type="button"
              role="tab"
              aria-selected={mode === "builder"}
              onclick={() => setMode("builder")}>Builder</button
            >
            <button
              type="button"
              role="tab"
              aria-selected={mode === "code"}
              disabled={mode === "builder" && !builderOk}
              onclick={() => setMode("code")}>Code</button
            >
          </div>
        </div>
        {#if mode === "builder"}
          <div class="query-builder">
            <div>
              <QueryExpr
                bind:node={root}
                {errors}
                {searchIndex}
                {kindIndex}
                {attrIndex}
                context="Root"
              />
            </div>
          </div>
        {:else}
          <div class="query-code-body">
            <textarea
              id="query-code-input"
              aria-label="Bazel query expression"
              aria-invalid={!!codeError}
              aria-describedby={codeWarning ? "query-code-message" : undefined}
              spellcheck="false"
              autocomplete="off"
              bind:value={draft}
              oninput={() => (codeError = null)}
            ></textarea>
            {#if codeWarning}
              <Notice id="query-code-message" tone={codeError ? "error" : "warning"}
                onaction={revertToBuilder} actionLabel="Revert to last Builder"
                >{codeError ?? "The edited query differs from the Builder expression."}</Notice
              >
            {/if}
          </div>
        {/if}
        <div class="query-shared">
          <div class="query-command">
            <pre class="query-command-pre"><code>{command}</code></pre>
            <button
              type="button"
              class="query-copy"
              aria-label="Copy command line"
              title="Copy command line"
              disabled={!valid}
              onclick={() => void copyToClipboard(command, "command")}
            >
              {#if copied === "command"}<Check size={14} />{:else}<Copy
                  size={14}
                />{/if}
            </button>
          </div>
          <div class="query-flags">
            <label class="query-check">
              <input type="checkbox" bind:checked={noImplicit} />
              <code>--noimplicit_deps</code>
            </label>
            <label class="query-check">
              <input type="checkbox" bind:checked={noTool} />
              <code>--notool_deps</code>
            </label>
            <div class="query-actions">
              {#if runBlockReason}
                <span class="query-busy dim">{runBlockReason}</span>
              {/if}
              <button
                type="button"
                class="button button-primary"
                disabled={!canRun}
                title={runBlockReason ?? "Execute query"}
                onclick={run}
                >{qstate.running ? "Executing…" : "Execute"}</button
              >
            </div>
          </div>
        </div>
      </section>
      <section class="query-panel" aria-labelledby="query-results-heading">
        <div class="query-results-head">
          <h2 id="query-results-heading">Results</h2>
          <div class="query-tabs" role="tablist" aria-label="Result view">
            <button
              type="button"
              role="tab"
              aria-selected={resultView === "raw"}
              onclick={() => (resultView = "raw")}>Raw</button
            >
            <button
              type="button"
              role="tab"
              aria-selected={resultView === "graph"}
              onclick={() => (resultView = "graph")}>Graph</button
            >
          </div>
          <span class="query-results-count">{resultCount}</span>
        </div>
        {#if qstate.error}
          <Notice tone="error">{qstate.error}</Notice>
        {/if}
        {#if failedOutput && !runningThisView}
          <Notice tone="error">
            <p>{failedOutput.summary}</p>
            {#if failedOutput.tail}<pre>{failedOutput.tail}</pre>{/if}
          </Notice>
        {/if}
        {#if graphWarn && !stored?.truncated}
          <Notice tone="warning"><pre>{graphWarn}</pre></Notice>
        {/if}
        {#if stored?.truncated && !failedOutput && !runningThisView}
          <Notice tone={resultView === "graph" ? "error" : "warning"}>
            {#if resultView === "graph"}Bazel truncated the graph output — it
              cannot be rendered.{:else}Bazel truncated this output.{/if}
          </Notice>
        {/if}
        <div class="query-results-body">
          {#if runningThisView}
            <p class="query-empty" role="status">Executing…</p>
          {:else if stored && !failedOutput}
            {#if resultView === "raw"}
              {#if rawLines.length === 0}
                <p class="query-empty">No targets matched.</p>
              {:else}
                <div class="query-raw-wrap">
                  <pre class="query-raw">{rawShown.join("\n")}</pre>
                  <button
                    type="button"
                    class="query-copy"
                    aria-label="Copy output"
                    title="Copy output"
                    onclick={() => {
                      if (stored) void copyToClipboard(stored.stdout, "output");
                    }}
                  >
                    {#if copied === "output"}<Check size={14} />{:else}<Copy
                        size={14}
                      />{/if}
                  </button>
                </div>
                {#if rawMore > 0}
                  <p class="query-empty">
                    … {rawMore.toLocaleString()} more lines not shown.
                  </p>
                {/if}
              {/if}
            {:else if stored.truncated}
              <p class="query-empty">No graph to render.</p>
            {/if}
          {:else if failedOutput}
            <p class="query-empty">The last query failed — see the error above.</p>
          {:else if busyReason}
            <p class="query-empty">Waiting — {busyReason}.</p>
          {:else}
            <p class="query-empty">Select Execute to fetch results.</p>
          {/if}
          <QueryGraph dot={graphDot} />
        </div>
      </section>
    </div>
  </div>
</section>
<div role="status" aria-live="polite" class="query-feedback" class:visible={!!feedback}
  >{feedback}</div
>
