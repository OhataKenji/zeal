<script lang="ts" module>
  import type { Viz } from "@viz-js/viz";

  // The Graphviz wasm is only fetched when a graph result is rendered, and the
  // module promise is shared across remounts.
  let vizInstance: Promise<Viz> | undefined;
  function loadViz() {
    return (vizInstance ??= import("@viz-js/viz").then((m) => m.instance()));
  }
</script>

<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onDestroy } from "svelte";
  import { app } from "./state.svelte";
  import QueryExpr from "./QueryExpr.svelte";
  import {
    applyExample,
    codeDiverged,
    commandLine,
    createQuery,
    DEFAULT_FLAGS,
    defaultNodeFor,
    emptyQueryState,
    EXAMPLES,
    keyFor,
    parse,
    serialize,
    validate,
    type CommandFlags,
    type Expr,
    type QueryState,
  } from "./query";
  import type { Snapshot } from "./types";

  let { snap, workspace }: { snap: Snapshot | null; workspace: string } = $props();

  const labelsId = "query-labels";
  const attrsId = "query-attrs";
  const kindsId = "query-kinds";

  const wsName = $derived(
    workspace.split(/[\\/]/).filter(Boolean).at(-1) ?? workspace,
  );
  const labelOptions = $derived(
    (snap?.targets ?? []).slice(0, 300).map((t) => t.label),
  );
  const attrOptions = $derived(
    [...new Set((snap?.edges ?? []).map((e) => e.attr))].sort(),
  );
  const kindOptions = $derived([
    ".*_test rule",
    ...[...new Set((snap?.targets ?? []).map((t) => t.ruleClass))]
      .sort()
      .map((rc) => (rc.endsWith("file") ? rc : `${rc} rule`)),
  ]);
  // Serialized forms of the examples, for the aria-current highlight. The mock
  // only marks an example current under default flags and Builder mode.
  const exampleTexts = new Map(
    EXAMPLES.map((ex) => {
      const s = serialize(ex.build());
      return [ex.id, s.ok ? s.text : null] as const;
    }),
  );

  // Nodes that are empty targets render as holes unless their id was marked a
  // literal input when the kind selector picked "Target pattern".
  const literalEditors = new Set<number>();
  let root = $state<Expr>(
    EXAMPLES[0]?.build() ?? defaultNodeFor("target"),
  );
  let mode = $state<"builder" | "code">("builder");
  let draft = $state("");
  let codeError = $state<string | null>(null);
  let noImplicit = $state(DEFAULT_FLAGS.noImplicit);
  let noTool = $state(DEFAULT_FLAGS.noTool);
  let resultView = $state<"raw" | "graph">("raw");
  // The result view the in-flight run belongs to; cleared when it settles.
  let runningView = $state<"raw" | "graph" | null>(null);
  // Armed-confirm replaces window.confirm (WKWebView has no dialogs).
  let armed = $state<
    { kind: "example"; id: string } | { kind: "revert" } | null
  >(null);
  let armTimer: ReturnType<typeof setTimeout> | undefined;
  let feedback = $state("");
  let fbTimer: ReturnType<typeof setTimeout> | undefined;
  let qstate = $state<QueryState>(emptyQueryState());
  let controller = $state<ReturnType<typeof createQuery> | null>(null);
  let graphUrl = $state<string | null>(null);
  let graphError = $state<string | null>(null);
  let viewEl = $state<HTMLElement>();

  $effect(() => {
    const next = createQuery({ invoke }, (s) => (qstate = s));
    controller = next;
    return next.dispose;
  });
  $effect(() => {
    if (!qstate.running) runningView = null;
  });
  onDestroy(() => {
    clearTimeout(armTimer);
    clearTimeout(fbTimer);
    if (graphUrl) URL.revokeObjectURL(graphUrl);
  });

  const st = $derived(app.state);
  const flags = $derived<CommandFlags>({ noImplicit, noTool });
  const serialized = $derived(serialize(root));
  const builderOk = $derived(serialized.ok);
  const builderText = $derived(serialized.ok ? serialized.text : "");
  const exprText = $derived(mode === "code" ? draft : builderText);
  const errors = $derived(
    mode === "builder" ? validate(root) : new Map<string, string>(),
  );
  const generated = $derived(
    serialized.ok ? serialized.text : `(serialize error) ${serialized.error}`,
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

  // Runs serialize with build/test and ingest in the core; the reason text
  // doubles as the disabled-Run explanation.
  const busyReason = $derived.by((): string | null => {
    if (!app.view.ready) return "not connected to Bazel";
    if (app.view.opening) return "the workspace is changing";
    if (app.view.starting) return "a run is starting";
    if (st?.running) return "a build/test run is in progress";
    if (st?.ingest === "running") return "targets are loading";
    if (st?.query === "running" && !qstate.running)
      return "a query is already running";
    return null;
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

  const stored = $derived(qstate.results[resultView]);
  const currentKey = $derived(
    exprText.trim() === "" ? null : keyFor(exprText, resultView, flags),
  );
  const stale = $derived(stored !== undefined && stored.key !== currentKey);
  const runningThisView = $derived(qstate.running && runningView === resultView);
  const failedOutput = $derived.by(() => {
    if (!stored) return null;
    if (stored.timedOut)
      return { summary: "Query timed out.", tail: stored.stderrTail };
    if (stored.stopped)
      return { summary: "Query stopped before it finished.", tail: stored.stderrTail };
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
  const canCopyOutput = $derived(!!stored && stored.stdout !== "");
  // A successful graph run can still drop nodes (`--graph:node_limit`); bazel
  // only says so on stderr.
  const graphWarn = $derived.by(() => {
    if (resultView !== "graph" || !stored || failedOutput || qstate.running)
      return null;
    const lines = stored.stderrTail
      .split("\n")
      .filter((l) => l.startsWith("WARNING"));
    return lines.length ? lines.join("\n") : null;
  });

  // A successful graph run's DOT becomes an SVG blob URL; Graphviz output is
  // never appended to the DOM (CSP is unset and SVG links are an injection
  // surface).
  const graphDot = $derived(
    resultView === "graph" &&
      stored &&
      !failedOutput &&
      !stored.truncated &&
      stored.stdout.trim() !== ""
      ? stored.stdout
      : null,
  );
  $effect(() => {
    const dot = graphDot;
    graphError = null;
    if (!dot) return;
    let cancelled = false;
    loadViz()
      .then((viz) => {
        if (cancelled) return;
        let svg: string;
        try {
          svg = viz.renderString(dot, { format: "svg" });
        } catch (e) {
          if (!cancelled)
            graphError = `Graphviz could not render this result: ${String(e)}`;
          return;
        }
        if (!cancelled)
          graphUrl = URL.createObjectURL(
            new Blob([svg], { type: "image/svg+xml" }),
          );
      })
      .catch((e) => {
        // Let a later render retry: a rejected promise must not be cached.
        vizInstance = undefined;
        if (!cancelled)
          graphError = `Could not load the graph renderer: ${String(e)}`;
      });
    return () => {
      cancelled = true;
      if (graphUrl) {
        URL.revokeObjectURL(graphUrl);
        graphUrl = null;
      }
    };
  });

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
    clearPending();
    root = parsed.expr;
    codeError = null;
    mode = "builder";
  }

  // A view switch that wanted to run but was gated retries when the slot
  // frees; only switches remember a pending run, never stale-key edits.
  let pendingKey = $state<string | null>(null);
  function setResultView(view: "raw" | "graph") {
    if (view === resultView) return;
    resultView = view;
    // A view whose result is missing or fetched under another key runs on
    // entry, under the same gates as the Run button.
    const s = qstate.results[view];
    const key = exprText.trim() === "" ? null : keyFor(exprText, view, flags);
    if (canRun && key !== null && s?.key !== key) {
      run();
      return;
    }
    if (s === undefined && key !== null) pendingKey = key;
  }
  $effect(() => {
    if (pendingKey && pendingKey === currentKey && canRun) {
      pendingKey = null;
      run();
    }
  });
  // Any edit to the expression or flags voids an armed auto-run; example
  // loads and view switches arm it only after their own edits are done.
  function clearPending() {
    pendingKey = null;
  }

  function run() {
    if (!canRun) return;
    // Any actual run fulfills an armed auto-run intent; leaving it armed
    // would fire a duplicate when this run frees the gate again.
    pendingKey = null;
    runningView = resultView;
    void controller?.run(exprText, resultView, flags);
  }

  // Mirrors the core's `validate_expr` so Run is never enabled for an
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

  function stop() {
    void controller?.stop();
  }

  function disarm() {
    armed = null;
    clearTimeout(armTimer);
  }

  function armFor(token: { kind: "example"; id: string } | { kind: "revert" }) {
    armed = token;
    clearTimeout(armTimer);
    armTimer = setTimeout(() => (armed = null), 4000);
  }

  function loadExample(id: string) {
    const needsConfirm =
      mode === "code" &&
      codeDiverged(root, draft) &&
      !(armed?.kind === "example" && armed.id === id);
    if (needsConfirm) {
      armFor({ kind: "example", id });
      return;
    }
    const built = applyExample(id);
    if (!built) return;
    disarm();
    root = built;
    mode = "builder";
    draft = "";
    codeError = null;
    noImplicit = DEFAULT_FLAGS.noImplicit;
    noTool = DEFAULT_FLAGS.noTool;
    if (canRun) {
      run();
    } else {
      pendingKey = currentKey;
    }
  }

  function revertToBuilder() {
    if (armed?.kind !== "revert") {
      armFor({ kind: "revert" });
      return;
    }
    disarm();
    clearPending();
    draft = builderText;
    codeError = null;
    mode = "builder";
  }

  function setFeedback(msg: string) {
    feedback = msg;
    clearTimeout(fbTimer);
    fbTimer = setTimeout(() => (feedback = ""), 3000);
  }

  async function copyText(text: string, okMsg: string) {
    try {
      await navigator.clipboard.writeText(text);
      setFeedback(okMsg);
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

  function exampleCurrent(id: string): boolean {
    return (
      mode === "builder" &&
      noImplicit === DEFAULT_FLAGS.noImplicit &&
      noTool === DEFAULT_FLAGS.noTool &&
      builderOk &&
      builderText === exampleTexts.get(id)
    );
  }
</script>

<svelte:window onkeydown={onKey} />

<section class="query-view" bind:this={viewEl}>
  <aside class="query-rail">
    <h2 class="query-rail-heading">Examples</h2>
    <div class="query-examples">
      {#each EXAMPLES as example (example.id)}
        <button
          type="button"
          class="query-example"
          aria-current={exampleCurrent(example.id) ? "true" : undefined}
          onclick={() => loadExample(example.id)}
        >
          <code>{example.name}</code>
          {#if armed?.kind === "example" && armed.id === example.id}
            <span class="query-ex-desc"
              >Discard Code edits — click again to load</span
            >
          {:else if example.desc}<span class="query-ex-desc">{example.desc}</span>{/if}
        </button>
      {/each}
    </div>
    <p class="query-conn-note">Runs a <code>bazel query</code> in {wsName}</p>
  </aside>
  <div class="query-main">
    <div class="query-content">
      <header class="query-page-header">
        <div class="query-crumbs">{wsName}</div>
        <div class="query-title-row"><h1>Query editor</h1></div>
      </header>
      <section class="query-panel" aria-labelledby="query-heading">
        <div class="query-panel-head">
          <h2 id="query-heading">Bazel query</h2>
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
                bind:node={
                  () => root,
                  (v) => {
                    clearPending();
                    root = v;
                  }
                }
                {errors}
                {literalEditors}
                {labelsId}
                {attrsId}
                {kindsId}
                context="Root"
              />
            </div>
            <datalist id={labelsId}>
              {#each labelOptions as label (label)}
                <option value={label}></option>
              {/each}
            </datalist>
            <datalist id={attrsId}>
              {#each attrOptions as attr (attr)}
                <option value={attr}></option>
              {/each}
            </datalist>
            <datalist id={kindsId}>
              {#each kindOptions as pattern (pattern)}
                <option value={pattern}></option>
              {/each}
            </datalist>
            <div class="query-gen">
              <p class="query-gen-label">Generated query</p>
              <pre><code>{generated}</code></pre>
            </div>
          </div>
        {:else}
          <div class="query-code-body">
            <label for="query-code-input">Bazel query expression</label>
            <textarea
              id="query-code-input"
              spellcheck="false"
              autocomplete="off"
              bind:value={draft}
              oninput={() => {
                codeError = null;
                clearPending();
              }}
            ></textarea>
            <p class="query-hint">
              Switching to Builder parses supported syntax and normalizes
              parentheses and operators.
            </p>
            {#if codeWarning}
              <div class="query-code-warning" class:is-error={!!codeError}>
                <span
                  >{codeError ??
                    "The edited query differs from the Builder expression."}</span
                >
                <button
                  type="button"
                  class="button"
                  onclick={revertToBuilder}
                  >{armed?.kind === "revert"
                    ? "Discard Code edits?"
                    : "Revert to last Builder"}</button
                >
              </div>
            {/if}
          </div>
        {/if}
        <div class="query-shared">
          <details class="query-command-details" open>
            <summary>Command</summary>
            <p class="query-command-line"><code>{command}</code></p>
          </details>
          <div class="query-flags">
            <label class="query-check">
              <input
                type="checkbox"
                bind:checked={noImplicit}
                onchange={clearPending}
              />
              <code>--noimplicit_deps</code>
            </label>
            <label class="query-check">
              <input
                type="checkbox"
                bind:checked={noTool}
                onchange={clearPending}
              />
              <code>--notool_deps</code>
            </label>
            <div class="query-actions">
              {#if runBlockReason}
                <span class="query-busy dim">{runBlockReason}</span>
              {/if}
              <button
                type="button"
                class="button"
                disabled={!valid}
                onclick={() => void copyText(command, "Command copied")}
                >Copy command</button
              >
              <button
                type="button"
                class="button"
                disabled={!canCopyOutput}
                onclick={() => {
                  if (stored) void copyText(stored.stdout, "Output copied");
                }}
                >Copy output</button
              >
              {#if qstate.running}
                <button
                  type="button"
                  class="button"
                  disabled={qstate.stopping || st?.queryStopping}
                  onclick={stop}
                  >{qstate.stopping || st?.queryStopping
                    ? "Stopping…"
                    : "Stop"}</button
                >
              {/if}
              <button
                type="button"
                class="button button-primary"
                disabled={!canRun}
                title={runBlockReason ?? "Run query"}
                onclick={run}
                >{qstate.running ? "Running…" : "Run"}</button
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
              onclick={() => setResultView("raw")}>Raw</button
            >
            <button
              type="button"
              role="tab"
              aria-selected={resultView === "graph"}
              onclick={() => setResultView("graph")}>Graph</button
            >
          </div>
          <span class="query-results-count">{resultCount}</span>
        </div>
        {#if stale}
          <p class="query-stale">Query changed — showing previous results</p>
        {/if}
        {#if qstate.error}
          <p class="query-strip is-error" role="alert">{qstate.error}</p>
        {/if}
        {#if failedOutput && !runningThisView}
          <div class="query-strip is-error" role="alert">
            <p>{failedOutput.summary}</p>
            {#if failedOutput.tail}<pre>{failedOutput.tail}</pre>{/if}
          </div>
        {/if}
        {#if graphWarn && !stored?.truncated}
          <pre class="query-strip is-warn">{graphWarn}</pre>
        {/if}
        {#if stored?.truncated && !failedOutput && !runningThisView}
          <p
            class="query-strip"
            class:is-error={resultView === "graph"}
            class:is-warn={resultView !== "graph"}
          >
            {#if resultView === "graph"}Bazel truncated the graph output — it
              cannot be rendered.{:else}Bazel truncated this output.{/if}
          </p>
        {/if}
        {#if graphError}
          <p class="query-strip is-error" role="alert">{graphError}</p>
        {/if}
        <div class="query-results-body">
          {#if runningThisView}
            <p class="query-empty" role="status">Running…</p>
          {:else if stored && !failedOutput}
            {#if resultView === "raw"}
              {#if rawLines.length === 0}
                <p class="query-empty">No targets matched.</p>
              {:else}
                <pre class="query-raw">{rawShown.join("\n")}</pre>
                {#if rawMore > 0}
                  <p class="query-empty">
                    … {rawMore.toLocaleString()} more lines not shown.
                  </p>
                {/if}
              {/if}
            {:else if stored.truncated}
              <p class="query-empty">No graph to render.</p>
            {:else if graphError}
              <pre class="query-raw">{stored.stdout}</pre>
            {:else if graphUrl}
              <img
                class="query-graph"
                src={graphUrl}
                alt="Dependency graph for the query"
              />
            {:else if stored.stdout.trim() === ""}
              <p class="query-empty">No graph output.</p>
            {:else}
              <p class="query-empty" role="status">Rendering graph…</p>
            {/if}
          {:else if failedOutput}
            <p class="query-empty">The last run failed — see the error above.</p>
          {:else if busyReason}
            <p class="query-empty">Waiting — {busyReason}.</p>
          {:else}
            <p class="query-empty">Select Run to fetch results.</p>
          {/if}
        </div>
      </section>
    </div>
  </div>
</section>
<div role="status" aria-live="polite" class="query-feedback" class:visible={!!feedback}
  >{feedback}</div
>
