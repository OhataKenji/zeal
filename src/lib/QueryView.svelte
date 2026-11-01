<script lang="ts" module>
  import VizWorker from "./viz-worker?worker";

  // The Graphviz wasm is only fetched when a graph result is rendered. The
  // worker is shared across remounts; a hung layout is abandoned by
  // terminating it, which also drops the next render's warm instance.
  let vizWorker: Worker | undefined;
  let vizBusy = false;
  const VIZ_TIMEOUT = 30_000;

  function renderDot(
    dot: string,
    ok: (svg: string) => void,
    fail: (error: string) => void,
  ): () => void {
    if (vizBusy) {
      vizWorker?.terminate();
      vizWorker = undefined;
      vizBusy = false;
    }
    const w = (vizWorker ??= new VizWorker());
    vizBusy = true;
    const timer = setTimeout(() => {
      kill();
      fail("Graphviz could not lay out this result within 30s.");
    }, VIZ_TIMEOUT);
    w.onmessage = (e) => {
      settle();
      const d = e.data as { svg?: string; error?: string };
      if (d.error) fail(d.error);
      else ok(d.svg ?? "");
    };
    w.onerror = (e) => {
      settle();
      fail(e.message || "the graph renderer crashed");
    };
    function settle() {
      clearTimeout(timer);
      vizBusy = false;
      w.onmessage = null;
      w.onerror = null;
    }
    function kill() {
      settle();
      if (vizWorker === w) {
        w.terminate();
        vizWorker = undefined;
      }
    }
    w.postMessage(dot);
    // Abandoning a render means killing the worker: the queued DOT would
    // otherwise delay the next layout by whatever time it still needs.
    return () => {
      if (vizBusy && vizWorker === w) kill();
    };
  }
</script>

<script lang="ts">
  import Notice from "./Notice.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { Check, Copy } from "phosphor-svelte";
  import { onDestroy, untrack } from "svelte";
  import { app } from "./state.svelte";
  import { createIndex, labelTarget } from "./search";
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
    parse,
    serialize,
    validate,
    type CommandFlags,
    type Expr,
    type QueryState,
  } from "./query";
  import type { TargetIndex } from "./types";

  let { snap }: { snap: TargetIndex | null } = $props();

  const searchIndex = $derived(
    snap ? createIndex(snap.targets) : null,
  );
  // Attribute names remain free text; the tree index carries no attributes.
  const attrIndex = null;
  // kind()'s pattern arg searches a wildcard-test hint plus every rule class
  // in the snapshot.
  const kindIndex = $derived(
    snap
      ? createIndex(
          [
            ".*_test rule",
            ...[...new Set(snap.targets.map((t) => t.ruleClass))]
              .sort()
              .map((rc) => (rc.endsWith("file") ? rc : `${rc} rule`)),
          ].map((label) => labelTarget(label, "kind")),
        )
      : null,
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
  let graphUrl = $state<string | null>(null);
  let graphError = $state<string | null>(null);
  let graphZoom = $state(1);
  let graphDialog = $state<HTMLDialogElement>();
  let graphCanvas = $state<HTMLDivElement>();
  let viewEl = $state<HTMLElement>();

  $effect(() => {
    const next = createQuery({ invoke }, (s) => (qstate = s));
    controller = next;
    return next.dispose;
  });
  onDestroy(() => {
    clearTimeout(fbTimer);
    clearTimeout(copyTimer);
    vizWorker?.terminate();
    vizWorker = undefined;
    vizBusy = false;
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
    void exprText;
    void resultView;
    void flags;
    untrack(() => controller?.clear());
  });

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

  // Editing the query clears the result rather than marking it stale.
  const stored = $derived(qstate.result);
  // Only one run is ever in flight; which view started it does not matter —
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
    const abandon = renderDot(
      dot,
      (svg) => {
        graphUrl = URL.createObjectURL(
          new Blob([svg], { type: "image/svg+xml" }),
        );
      },
      (error) => {
        graphError = error;
      },
    );
    return () => {
      abandon();
      if (graphDialog?.open) graphDialog.close();
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
    root = parsed.expr;
    codeError = null;
    mode = "builder";
  }

  function run() {
    if (!canRun) return;
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
  <aside class="query-rail">
    <h2 class="query-rail-heading">Example</h2>
    <div class="query-examples">
      {#each EXAMPLES as example (example.id)}
        <button
          type="button"
          class="query-example"
          onclick={() => loadExample(example.id)}
        >
          <code>{example.name}</code>
          <span class="query-ex-desc">{example.desc}</span>
        </button>
      {/each}
    </div>
  </aside>
  <div class="query-main">
    <div class="query-content">
      <header class="query-page-header">
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
              aria-label="Copy command"
              title="Copy command"
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
        {#if graphError}
          <Notice tone="error">{graphError}</Notice>
        {/if}
        <div class="query-results-body">
          {#if runningThisView}
            <p class="query-empty" role="status">Running…</p>
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
            {:else if graphError}
              <pre class="query-raw">{stored.stdout}</pre>
            {:else if graphUrl}
              <div class="query-graph-actions">
                <button
                  type="button"
                  class="button"
                  onclick={() => {
                    graphZoom = 1;
                    graphDialog?.showModal();
                    graphCanvas?.focus();
                  }}>Open viewer</button
                >
              </div>
              <img
                class="query-graph"
                src={graphUrl}
                alt="Dependency graph for the query"
              />
              <dialog
                class="query-graph-dialog"
                aria-label="Query dependency graph viewer"
                bind:this={graphDialog}
              >
                <div class="query-graph-toolbar">
                  <span>Graph</span>
                  <button
                    type="button"
                    class="button"
                    aria-label="Zoom out"
                    disabled={graphZoom <= 0.25}
                    onclick={() => (graphZoom = Math.max(0.25, graphZoom - 0.25))}
                    >−</button
                  >
                  <button
                    type="button"
                    class="button"
                    title="Reset to original size"
                    onclick={() => (graphZoom = 1)}
                    >{Math.round(graphZoom * 100)}%</button
                  >
                  <button
                    type="button"
                    class="button"
                    aria-label="Zoom in"
                    disabled={graphZoom >= 4}
                    onclick={() => (graphZoom = Math.min(4, graphZoom + 0.25))}
                    >+</button
                  >
                  <button
                    type="button"
                    class="button"
                    onclick={() => graphDialog?.close()}>Close</button
                  >
                </div>
                <!-- svelte-ignore a11y_no_noninteractive_tabindex (The scroll container needs focus for keyboard scrolling.) -->
                <div
                  class="query-graph-canvas"
                  role="region"
                  aria-label="Graph canvas"
                  tabindex="0"
                  bind:this={graphCanvas}
                >
                  <img
                    src={graphUrl}
                    alt="Dependency graph for the query"
                    style:zoom={graphZoom}
                  />
                </div>
              </dialog>
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
