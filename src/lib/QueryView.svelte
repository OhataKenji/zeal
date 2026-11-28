<script lang="ts">
  import VizWorker from "./viz-worker?worker";
  import { createGraphRenderer } from "./graph-renderer";
  import Notice from "./Notice.svelte";
  import { Bookmarks, CaretDown } from "phosphor-svelte";
  import { onDestroy, untrack } from "svelte";
  import { createIndex, type SearchIndex } from "./search";
  import CopyButton from "./CopyButton.svelte";
  import QueryExpr from "./QueryExpr.svelte";
  import TargetActions from "./TargetActions.svelte";
  import {
    attrCandidates,
    codeDiverged,
    commandLine,
    DEFAULT_FLAGS,
    EXAMPLES,
    kindCandidates,
    parse,
    parseQuery,
    outputFormatFor,
    parseCqueryOutput,
    producerQuery,
    queryInputs,
    serialize,
    validate,
    type CommandFlags,
    type Expr,
    type QueryDraft,
    type QueryInputs,
    type QueryState,
    type QueryKind,
    type ResultView,
  } from "./query";

  let {
    targetIndex,
    bazelAvailable,
    query: qstate,
    initialDraft,
    ondraftchange,
    onselectquery,
    onrunquery,
    onselecttarget,
  }: {
    targetIndex: SearchIndex | null;
    bazelAvailable: boolean;
    query: QueryState;
    initialDraft: QueryDraft;
    ondraftchange?: (draft: QueryDraft) => void;
    onselectquery?: (request: QueryInputs) => void;
    onrunquery?: (request: QueryInputs) => void;
    onselecttarget?: (label: string) => void;
  } = $props();

  // Attribute names come from a static hint list: the label_kind snapshot
  // carries no attributes, and completion only assists.
  const attrIndex = createIndex(attrCandidates());
  // kind()'s pattern arg searches a wildcard-test hint, the non-rule kinds,
  // and every rule class in the snapshot.
  const kindIndex = $derived(
    targetIndex ? createIndex(kindCandidates(targetIndex.targets)) : null,
  );
  const knownTargets = $derived(new Set((targetIndex?.targets ?? []).map((t) => t.label)));
  const initial = untrack(() => initialDraft);
  let root = $state<Expr>(structuredClone(initial.root));
  let mode = $state<"builder" | "code">(initial.mode);
  let draft = $state(initial.draft);
  let codeError = $state<string | null>(initial.codeError);
  let noImplicit = $state(initial.noImplicit);
  let noTool = $state(initial.noTool);
  let kind = $state<QueryKind>(initial.kind);
  let buildFlags = $state(initial.buildFlags);
  let transitions = $state(initial.transitions);
  let resultView = $state<ResultView>(initial.resultView);
  const outputFormat = $derived(outputFormatFor(kind, resultView, transitions));
  const phases = {
    query: "Loading — explore target dependencies before configuration.",
    cquery: "Analysis — inspect targets and their configurations.",
    aquery: "Action Graph — inspect build actions and trace their inputs.",
  };
  const kinds: QueryKind[] = ["query", "cquery", "aquery"];
  let feedback = $state("");
  let fbTimer: ReturnType<typeof setTimeout> | undefined;
  let graphUrl = $state<string | null>(null);
  let graphError = $state<string | null>(null);
  let graphZoom = $state(1);
  let graphDialog = $state<HTMLDialogElement>();
  let graphCanvas = $state<HTMLDivElement>();
  let viewEl = $state<HTMLElement>();
  let detailsEl = $state<HTMLDetailsElement>();

  // Each mounted view owns its lazy Graphviz worker and reuses it between layouts.
  const renderer = createGraphRenderer(() => new VizWorker());
  onDestroy(() => {
    clearTimeout(fbTimer);
    renderer.dispose();
    if (graphUrl) URL.revokeObjectURL(graphUrl);
  });

  function resolveTargetLink(raw: string): string | null {
    if (!onselecttarget) return null;
    const trimmed = raw.trim();
    const key = trimmed.startsWith("@@//")
      ? trimmed.slice(2)
      : trimmed.startsWith("@//")
        ? trimmed.slice(1)
        : trimmed;
    return knownTargets.has(key) ? key : null;
  }

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
    // Persist workspace-scoped editor state and notify the controller when
    // inputs change. Table and Raw share an execution for cquery and aquery.
    const nextDraft: QueryDraft = {
      kind,
      root: $state.snapshot(root),
      mode,
      draft,
      codeError,
      noImplicit,
      noTool,
      buildFlags,
      transitions,
      resultView,
    };
    const next = request;
    untrack(() => {
      ondraftchange?.(nextDraft);
      onselectquery?.(next);
    });
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
  const resultCount = $derived.by(() => {
    if (!stored || failedOutput || qstate.running) return "";
    if (resultView === "table" && kind === "cquery")
      return `${rows.length} configured target${rows.length === 1 ? "" : "s"}`;
    if (resultView !== "raw") return "";
    return `${rawLines.length} ${kind === "query" ? "target" : "line"}${rawLines.length === 1 ? "" : "s"}`;
  });
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
    const abandon = renderer.render(
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
    onrunquery?.(request);
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
  function loadExample(expr: string) {
    root = parseQuery(expr, kind);
    mode = "builder";
    draft = "";
    codeError = null;
    noImplicit = DEFAULT_FLAGS.noImplicit;
    noTool = DEFAULT_FLAGS.noTool;
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
      <div class="view-head">
        <h1 class="view-title" id="query-heading">Query editor</h1>
      </div>
      <div class="tabs" role="tablist" aria-label="Bazel graph">
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
              {#each Object.entries(EXAMPLES[kind]) as [title, examples]}
                <div class="query-examples-group">
                  <div class="query-examples-group-title">{title}</div>
                  {#each examples as example}
                    <button
                      type="button"
                      class="query-example-item"
                      role="menuitem"
                      onclick={() => loadExample(example.expr)}
                    >
                      <code>{example.name}</code>
                      <span class="query-ex-desc">{example.desc}</span>
                    </button>
                  {/each}
                </div>
              {/each}
            </div>
          </details>
          <div class="sub-switch" role="tablist" aria-label="Query mode">
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
              searchIndex={targetIndex}
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
          <div class="sub-switch" role="tablist" aria-label="Result view">
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
        {#if failedOutput && !qstate.running}
          <Notice tone="error">
            <p>{failedOutput.summary}</p>
            {#if failedOutput.tail}<pre>{failedOutput.tail}</pre>{/if}
          </Notice>
        {/if}
        {#if graphWarn && !stored?.truncated}
          <Notice tone="warning"><pre>{graphWarn}</pre></Notice>
        {/if}
        {#if stored && !failedOutput && !qstate.running &&
          (resultView === "raw" ? stored.stdoutTruncated : stored.truncated)}
          <Notice tone={resultView === "graph" ? "error" : "warning"}>
            {#if resultView === "raw"}Showing a truncated preview of the output.{:else if resultView === "graph"}Bazel truncated the graph output — it
              cannot be rendered.{:else if kind === "aquery"}Action data exceeds 32 MiB. Narrow the expression and execute again.{:else}Bazel truncated this output.{/if}
          </Notice>
        {/if}
        {#if graphError}
          <Notice tone="error">{graphError}</Notice>
        {/if}
        <div class="query-results-body">
          {#if qstate.running}
            <p class="query-empty" role="status">Executing…</p>
          {:else if stored && !failedOutput}
            {#if resultView === "raw"}
              {#if rawLines.length === 0}
                <p class="query-empty">{kind === "aquery" ? "No actions matched." : "No targets matched."}</p>
              {:else}
                <div class="query-raw-wrap">
                  {#if kind === "query" && onselecttarget}
                    <pre class="query-raw">{#each rawShown as line, i}{#if i > 0}{"\n"}{/if}{@const key = resolveTargetLink(line)}{#if key}<button type="button" class="table-link mono query-raw-link" onclick={() => onselecttarget?.(key)}>{line}</button>{:else}{line}{/if}{/each}</pre>
                  {:else}
                    <pre class="query-raw">{rawShown.join("\n")}</pre>
                  {/if}
                  <CopyButton
                    text={stored.stdout}
                    label="Copy output"
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
                <TargetActions actions={stored.actions} error={null} oninputclick={traceArtifactProducer} showOwner />
              {/if}
            {:else if resultView === "table"}
              {#if !rows.length}
                <p class="query-empty">No configured targets matched.</p>
              {:else}
                <table class="lens-table">
                  <thead><tr><th scope="col">Label</th><th scope="col">Configuration</th>{#if transitions}<th scope="col">Transitions</th>{/if}</tr></thead>
                  <tbody>
                    {#each rows.slice(0, RAW_LINE_CAP) as row}
                      {@const key = resolveTargetLink(row.label)}
                      <tr><td>{#if key}<button type="button" class="table-link mono" onclick={() => onselecttarget?.(key)}>{row.label}</button>{:else}<code>{row.label}</code>{/if}</td><td><code>{row.config}</code></td>
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
