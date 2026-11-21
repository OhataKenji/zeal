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
  import { Bookmarks, CaretDown } from "phosphor-svelte";
  import { onDestroy, onMount, untrack } from "svelte";
  import { createIndex } from "./search";
  import CopyButton from "./CopyButton.svelte";
  import QueryExpr from "./QueryExpr.svelte";
  import TargetActions from "./TargetActions.svelte";
  import {
    applyExample,
    attrCandidates,
    codeDiverged,
    commandLine,
    createQuery,
    DEFAULT_FLAGS,
    defaultNodeFor,
    emptyQueryState,
    EXAMPLES,
    kindCandidates,
    parse,
    outputFormatFor,
    parseCqueryOutput,
    producerQuery,
    queryInputs,
    serialize,
    validate,
    type CommandFlags,
    type Expr,
    type QueryState,
    type QueryKind,
    type ResultView,
  } from "./query";
  import type { TargetIndex } from "./types";

  let {
    snap,
    workspace,
    bazelAvailable,
    preset = null,
    onpresetapplied,
  }: {
    snap: TargetIndex | null;
    workspace: string;
    bazelAvailable: boolean;
    preset?: {kind: QueryKind; flags: string} | null;
    onpresetapplied?: () => void;
  } = $props();

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
  const initial = untrack(() => preset);
  let kind = $state<QueryKind>(initial?.kind ?? "query");
  let buildFlags = $state(initial?.flags ?? "");
  let transitions = $state(false);
  let resultView = $state<ResultView>(initial && initial.kind !== "query" ? "table" : "raw");
  onMount(() => {
    if (initial) onpresetapplied?.();
  });
  const outputFormat = $derived(outputFormatFor(kind, resultView, transitions));
  const phases = {
    query: "Loading — explore target dependencies before configuration.",
    cquery: "Analysis — inspect targets and their configurations.",
    aquery: "Action Graph — inspect build actions and trace their inputs.",
  };
  const kinds: QueryKind[] = ["query", "cquery", "aquery"];
  let feedback = $state("");
  let fbTimer: ReturnType<typeof setTimeout> | undefined;
  let qstate = $state<QueryState>(emptyQueryState());
  let controller = $state<ReturnType<typeof createQuery> | null>(null);
  let graphUrl = $state<string | null>(null);
  let graphError = $state<string | null>(null);
  const EXAMPLE_GROUPS = [
    { title: "Dependencies", ids: ["deps", "rdeps", "siblings", "buildfiles"] },
    { title: "Paths", ids: ["somepath", "allpaths"] },
    { title: "Filter & Attributes", ids: ["kindConstraint", "attrLabels"] },
    { title: "Set Operations", ids: ["osExceptKind", "depsIntersectKind"] },
  ] as const;
  const exampleGroups = $derived(kind === "query" ? EXAMPLE_GROUPS :
    [{ title: `${kind} examples`, ids: EXAMPLES.filter(e => e.kind === kind).map(e => e.id) }]);

  let graphZoom = $state(1);
  let graphDialog = $state<HTMLDialogElement>();
  let graphCanvas = $state<HTMLDivElement>();
  let viewEl = $state<HTMLElement>();
  let detailsEl = $state<HTMLDetailsElement>();

  $effect(() => {
    const next = untrack(() =>
      createQuery(workspace, { invoke }, (s) => (qstate = s)),
    );
    controller = next;
    return next.dispose;
  });
  onDestroy(() => {
    clearTimeout(fbTimer);
    vizWorker?.terminate();
    vizWorker = undefined;
    vizBusy = false;
    if (graphUrl) URL.revokeObjectURL(graphUrl);
  });

  function selectKind(next: QueryKind) {
    kind = next;
    resultView = next === "query" ? "raw" : "table";
    codeError = null;
  }

  const flags = $derived<CommandFlags>({ noImplicit, noTool, flags: buildFlags });
  const serialized = $derived(serialize(root));
  const builderOk = $derived(serialized.ok);
  const builderText = $derived(serialized.ok ? serialized.text : "");
  const exprText = $derived(mode === "code" ? draft : builderText);
  const errors = $derived(
    mode === "builder" ? validate(root, kind) : new Map<string, string>(),
  );
  const request = $derived(queryInputs(kind, exprText, outputFormat, flags));
  const command = $derived(
    mode === "builder" && !serialized.ok
      ? `(serialize error) ${serialized.error}`
      : commandLine(request),
  );
  const valid = $derived(
    mode === "builder" ? builderOk && errors.size === 0 : codeValid(draft),
  );
  const diverged = $derived(mode === "code" && codeDiverged(root, draft));
  const codeWarning = $derived(
    mode === "code" && (diverged || codeError !== null),
  );

  $effect.pre(() => {
    // Depend on the execution inputs, not the presentation: cquery's Table
    // and Raw are two views of the same stdout, even during an in-flight run.
    const next = request;
    untrack(() => controller?.select(next));
  });

  // Each editor owns one in-flight query for its workspace; other workspaces
  // run independently in the core.
  const busyReason = $derived(bazelAvailable ? null : "Bazel is not available");
  const canRun = $derived(valid && command !== "" && !busyReason && !qstate.running);
  const runBlockReason = $derived(
    qstate.running
      ? null
      : (busyReason ??
        (!valid
          ? mode === "builder"
            ? "fix the marked fields"
            : "enter a query expression"
          : command === "" ? "check flag quoting and escapes" : null)),
  );

  // Input changes clear the displayed result.
  const stored = $derived(qstate.result);
  // Only one invocation is ever in flight; which view started it does not matter —
  // either pane may as well show the spinner.
  const runningThisView = $derived(qstate.running);
  const failedOutput = $derived.by(() => {
    if (!stored) return null;
    if (stored.exitCode !== 0)
      return {
        summary: `Query failed${stored.exitCode === null ? "" : ` (exit ${stored.exitCode})`}.`,
        tail: stored.stderrTail,
      };
    return null;
  });

  const RAW_LINE_CAP = 5000;
  const rows = $derived(kind === "cquery" && resultView === "table" && stored ?
    parseCqueryOutput(stored.stdout) : []);
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
      ? `${rawLines.length} ${kind === "query" ? "target" : "line"}${rawLines.length === 1 ? "" : "s"}`
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

  // A successful graph invocation's DOT becomes an SVG blob URL; Graphviz output is
  // never appended to the DOM (SVG links are an injection surface).
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
    const parsed = parse(draft, kind);
    if (!parsed.ok) {
      codeError = parsed.error;
      return;
    }
    const errs = validate(parsed.expr, kind);
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
    void controller?.run(request);
  }

  function traceArtifactProducer(path: string, owner: string) {
    draft = producerQuery(path, owner);
    codeError = null;
    mode = "code";
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
    buildFlags = "";
    transitions = false;
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
      <h1 class="query-title" id="query-heading">Query editor</h1>
      <div class="query-tabs" role="tablist" aria-label="Bazel graph">
        {#each kinds as value}
          <button type="button" role="tab" aria-selected={kind === value} onclick={() => { if (kind !== value) selectKind(value); }}>{value}</button>
        {/each}
      </div>
      <p class="dim">{phases[kind]}</p>
      <section class="query-panel" aria-labelledby="query-heading">
        <div class="query-panel-head">
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
              {#each exampleGroups as group}
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
            <QueryExpr
              bind:node={root}
              queryKind={kind}
              {errors}
              {searchIndex}
              {kindIndex}
              {attrIndex}
              context="Root"
            />
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
            <pre class="query-command-pre"><code>{command || "Invalid flags: check quoting and escapes."}</code></pre>
            <CopyButton
              text={command}
              label="Copy command line"
              disabled={!valid || command === ""}
              onerror={setFeedback}
            />
          </div>
          <div class="query-flags">
            {#if kind === "query"}
            <label class="query-check">
              <input type="checkbox" bind:checked={noImplicit} />
              <code>--noimplicit_deps</code>
            </label>
            <label class="query-check">
              <input type="checkbox" bind:checked={noTool} />
              <code>--notool_deps</code>
            </label>
            {:else}
              <label>Build flags <input type="text" maxlength={16 * 1024} placeholder="--config=opt (use --flag=value)" bind:value={buildFlags} /></label>
              {#if kind === "cquery"}
                <label class="query-check"><input type="checkbox" bind:checked={transitions} /> Transitions</label>
              {/if}
            {/if}
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
              aria-selected={resultView !== "raw"}
              onclick={() => (resultView = kind === "query" ? "graph" : "table")}>{kind === "query" ? "Graph" : "Table"}</button
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
              cannot be rendered.{:else if outputFormat === "jsonproto"}Action data exceeds 32 MiB. Narrow the expression and execute again.{:else}Bazel truncated this output.{/if}
          </Notice>
        {/if}
        {#if graphError}
          <Notice tone="error">{graphError}</Notice>
        {/if}
        <div class="query-results-body">
          {#if runningThisView}
            <p class="query-empty" role="status">Executing…</p>
          {:else if stored && !failedOutput}
            {#if resultView === "raw"}
              {#if rawLines.length === 0}
                <p class="query-empty">{kind === "aquery" ? "No actions matched." : "No targets matched."}</p>
              {:else}
                <div class="query-raw-wrap">
                  <pre class="query-raw">{rawShown.join("\n")}</pre>
                  <CopyButton
                    text={stored.stdout}
                    label="Copy output"
                    onerror={setFeedback}
                  />
                </div>
                {#if rawMore > 0}
                  <p class="query-empty">
                    … {rawMore.toLocaleString()} more lines not shown.
                  </p>
                {/if}
              {/if}
            {:else if resultView === "table" && kind === "aquery"}
              {#if stored.actions}
                <TargetActions actions={stored.actions} error={null} oninputclick={traceArtifactProducer} />
              {/if}
            {:else if resultView === "table"}
              {#if !rows.length}
                <p class="query-empty">No configured targets matched.</p>
              {:else}
                <table>
                  <thead><tr><th scope="col">Label</th><th scope="col">Configuration</th>{#if transitions}<th scope="col">Transitions</th>{/if}</tr></thead>
                  <tbody>
                    {#each rows.slice(0, RAW_LINE_CAP) as row}
                      <tr><td><code>{row.label}</code></td><td><code>{row.config}</code></td>
                        {#if transitions}<td><pre>{row.transitions.join("\n")}</pre></td>{/if}
                      </tr>
                    {/each}
                  </tbody>
                </table>
                {#if rows.length > RAW_LINE_CAP}<p class="query-empty">… {(rows.length - RAW_LINE_CAP).toLocaleString()} more rows not shown.</p>{/if}
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
            <p class="query-empty">The last query failed — see the error above.</p>
          {:else if busyReason}
            <p class="query-empty">Waiting — {busyReason}.</p>
          {:else}
            <p class="query-empty">Select Execute to fetch results.</p>
          {/if}
        </div>
      </section>
    </div>
  </div>
</section>
<div role="status" aria-live="polite" class="query-feedback" class:visible={!!feedback}
  >{feedback}</div
>
