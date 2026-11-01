<script lang="ts">
  import type { SnapshotKey } from "fuzzysort";
  import { tick } from "svelte";
  import QueryExpr from "./QueryExpr.svelte";
  import SearchDropdown from "./SearchDropdown.svelte";
  import {
    keyAction,
    LIST_ID,
    moveIndex,
    optionId,
    search,
    type Match,
  } from "./search";
  import type { Target } from "./types";
  import {
    defaultNodeFor,
    FUNCTIONS,
    isExprHole,
    KIND_GROUPS,
    kindOf,
    switchKind,
    UNIVERSE_HELP,
    type ArgSpec,
    type Expr,
  } from "./query";

  let {
    node = $bindable(),
    errors,
    literalEditors,
    searchIndex,
    kindIndex,
    attrsId,
    context = "Root",
  }: {
    node: Expr;
    errors: Map<string, string>;
    literalEditors: Set<number>;
    searchIndex: SnapshotKey<Target> | null;
    kindIndex: SnapshotKey<Target> | null;
    attrsId: string;
    context?: string;
  } = $props();

  let wrap = $state<HTMLElement>();

  // An empty target is an unfilled expression slot until the user picks
  // "Target pattern" — the mark lives in `literalEditors`, not in the Expr.
  const hole = $derived(isExprHole(node, literalEditors));
  const kind = $derived(kindOf(node));
  const textError = $derived(errors.get(`${node.id}:text`));

  // The target field reuses the run composer's fuzzy find, including its
  // keyboard contract; the dropdown opens on typing or arrow keys.
  let targetOpen = $state(false);
  let targetIndex = $state(0);
  const targetText = $derived(node.kind === "target" ? node.text : "");
  const targetMatches = $derived(
    searchIndex ? search(searchIndex, targetText) : [],
  );
  const activeIdx = $derived(moveIndex(targetIndex, 0, targetMatches.length));
  const activeOpt = $derived(
    targetOpen && targetMatches.length > 0 ? optionId(activeIdx) : undefined,
  );

  // One key contract for every fuzzy combobox: arrows open/move, Enter picks,
  // Escape closes — keyAction decides the intent, this applies it.
  function comboKey(
    e: KeyboardEvent,
    ui: { open: boolean; index: number },
    matches: Match[],
    pick: (value: string) => void,
  ): { open: boolean; index: number } {
    if (e.isComposing) return ui;
    const action = keyAction(ui, e.key, matches);
    switch (action.kind) {
      case "move":
        e.preventDefault();
        return { open: true, index: action.index };
      case "pick":
        e.preventDefault();
        pick(action.value);
        return { open: false, index: 0 };
      case "close":
        return { open: false, index: ui.index };
      default:
        return ui;
    }
  }

  function pickTarget(value: string) {
    if (node.kind === "target") node.text = value;
    targetOpen = false;
    targetIndex = 0;
  }

  function targetKey(e: KeyboardEvent) {
    const ui = comboKey(
      e,
      { open: targetOpen, index: activeIdx },
      targetMatches,
      pickTarget,
    );
    targetOpen = ui.open;
    targetIndex = ui.index;
  }

  // kind()'s pattern arg gets the same combobox over rule-class candidates.
  let kindOpen = $state(false);
  let kindSel = $state(0);
  const kindArgI = $derived(
    node.kind === "call" && node.fn === "kind"
      ? FUNCTIONS.kind.args.findIndex((a) => a.name === "pattern")
      : -1,
  );
  const kindPattern = $derived(
    node.kind === "call" && kindArgI >= 0
      ? scalarValue(node.args[kindArgI])
      : "",
  );
  const kindMatches = $derived(
    kindIndex ? search(kindIndex, kindPattern) : [],
  );
  const kindActiveIdx = $derived(moveIndex(kindSel, 0, kindMatches.length));
  const kindActiveOpt = $derived(
    kindOpen && kindMatches.length > 0 ? optionId(kindActiveIdx) : undefined,
  );

  function fieldError(i: number): string | undefined {
    return errors.get(`${node.id}:${i}`);
  }

  function scalarValue(value: Expr | string | null): string {
    return value === null || value === undefined ? "" : String(value);
  }

  // Field ids mirror the mock's `f-<errKey>` naming.
  function fieldId(arg: string | number): string {
    return `f-${`${node.id}:${arg}`.replace(/[^A-Za-z0-9_-]/g, "_")}`;
  }

  function exprArg(call: Extract<Expr, { kind: "call" }>, i: number): Expr {
    // The catalog schema guarantees `expr` slots hold an Expr.
    return call.args[i] as Expr;
  }

  function datalist(arg: ArgSpec): string | undefined {
    if (arg.name === "name" || arg.name === "attr_name") return attrsId;
    return undefined;
  }

  function placeholder(arg: ArgSpec): string {
    if (arg.optional) return "optional";
    if (arg.type === "int") return "0";
    return arg.name === "pattern" ? "regex" : "word";
  }

  function focusFirst() {
    void tick().then(() => {
      if (!wrap) return;
      const el =
        wrap.querySelector<HTMLElement>(
          ".query-call-args :is(input, select)",
        ) ??
        wrap.querySelector<HTMLElement>(
          ".query-comp-body :is(input, select)",
        ) ??
        wrap.querySelector<HTMLElement>(".query-atom-row input") ??
        wrap.querySelector<HTMLElement>("input, select");
      el?.focus();
    });
  }

  function refocusKind() {
    void tick().then(() => {
      wrap
        ?.querySelector<HTMLElement>(`[data-kind-for="${node.id}"]`)
        ?.focus();
    });
  }

  // The hole selector always replaces the node; the node itself carries
  // nothing, so there is no switchKind carry-over.
  function chooseKind(e: Event) {
    const value = (e.currentTarget as HTMLSelectElement).value;
    const next = defaultNodeFor(value);
    if (next.kind === "target") literalEditors.add(next.id);
    node = next;
    focusFirst();
  }

  function changeKind(e: Event) {
    const value = (e.currentTarget as HTMLSelectElement).value;
    if (value === kind) return;
    const next = switchKind(node, value);
    if (next.kind === "target") literalEditors.add(next.id);
    // call→call and binary→binary keep the node id: refocus the selector.
    const sameId = next.id === node.id;
    node = next;
    if (sameId) refocusKind();
    else focusFirst();
  }

  function sigTitle(): string {
    if (node.kind === "call")
      return `Change expression type: ${FUNCTIONS[node.fn].signature}`;
    if (node.kind === "binary") return `Change expression type: expr ${node.op} expr2`;
    return "Change expression type";
  }
</script>

{#snippet kindOptions()}
  {#each KIND_GROUPS as [group, items] (group)}
    <optgroup label={group}>
      {#each items as [value, label] (value)}
        <option {value}>{label}</option>
      {/each}
    </optgroup>
  {/each}
{/snippet}

{#snippet kindSelect()}
  <select
    class="query-kind-select"
    data-kind-for={node.id}
    aria-label="Expression kind for {context}"
    title={sigTitle()}
    value={kind}
    onchange={changeKind}
  >
    {@render kindOptions()}
  </select>
{/snippet}

{#if hole}
  <div
    class="query-atom-row query-hole-row"
    data-node-id={node.id}
    data-kind="target"
    bind:this={wrap}
  >
    <select
      class="query-hole-select"
      class:query-invalid={!!textError}
      aria-label="Choose an expression for {context}"
      value=""
      onchange={chooseKind}
    >
      <option value="" disabled>Choose expression…</option>
      {@render kindOptions()}
    </select>
    {#if textError}<p class="query-field-err">Choose an expression</p>{/if}
  </div>
{:else if node.kind === "target"}
  {@const target = node as Extract<Expr, { kind: "target" }>}
  <div
    class="query-atom-row"
    data-node-id={node.id}
    data-kind="target"
    bind:this={wrap}
  >
    {@render kindSelect()}
    <div class="query-arg">
      <label for={fieldId("text")}>Target pattern</label>
      <div class="query-target-search">
        <input
          id={fieldId("text")}
          type="text"
          autocomplete="off"
          spellcheck="false"
          role="combobox"
          aria-expanded={targetOpen && !!searchIndex}
          aria-controls={targetOpen && searchIndex ? LIST_ID : undefined}
          aria-autocomplete="list"
          aria-activedescendant={activeOpt}
          placeholder="Fuzzy :all / //pkg:target"
          aria-label="{context} target pattern"
          class:query-invalid={!!textError}
          value={target.text}
          oninput={(e) => {
            target.text = e.currentTarget.value;
            // Editing to empty (Ctrl-k, select-all+Delete) must not collapse
            // the node into a hole and unmount this very input.
            if (target.text === "") literalEditors.add(node.id);
            targetIndex = 0;
            targetOpen = target.text !== "";
          }}
          onkeydown={targetKey}
          onblur={() => (targetOpen = false)}
        />
        {#if targetOpen && searchIndex}
          <SearchDropdown
            matches={targetMatches}
            index={activeIdx}
            onpick={pickTarget}
          />
        {/if}
      </div>
      {#if textError}<p class="query-field-err">{textError}</p>{/if}
    </div>
  </div>
{:else if node.kind === "call"}
  {@const schema = FUNCTIONS[node.fn]}
  {@const call = node as Extract<Expr, { kind: "call" }>}
  <div
    class="query-expr-card"
    data-node-id={node.id}
    data-kind="call"
    bind:this={wrap}
  >
    <div class="query-comp-head">{@render kindSelect()}</div>
    <div class="query-call-args">
      {#each schema.args as arg, i (i)}
        <div
          class="query-call-arg"
          class:is-wide={arg.type === "expr" &&
            exprArg(call, i).kind !== "target"}
          class:is-expr-simple={arg.type === "expr" &&
            exprArg(call, i).kind === "target"}
          class:is-word={arg.type === "word"}
          class:is-int={arg.type === "int"}
        >
          {#if arg.type === "expr"}
            <div class="query-arg-head">
              <span class="query-field-label">{arg.name}</span>
              {#if node.fn === "rdeps" && arg.name === "universe"}
                <details class="query-hint-details">
                  <summary aria-label="About {arg.name}">?</summary>
                  <p>{UNIVERSE_HELP}</p>
                </details>
              {/if}
            </div>
            <QueryExpr
              bind:node={
                () => exprArg(call, i),
                (value: Expr) => (call.args[i] = value)
              }
              {errors}
              {literalEditors}
              {searchIndex}
              {attrsId}
              {kindIndex}
              context="{node.fn} {arg.name}"
            />
          {:else}
            {@const argError = fieldError(i)}
            <div class="query-arg">
              <label for={fieldId(i)}
                >{arg.name}{arg.optional ? " (optional)" : ""}</label
              >
              {#if node.fn === "kind" && arg.name === "pattern"}
                <div class="query-target-search">
                  <input
                    id={fieldId(i)}
                    type="text"
                    autocomplete="off"
                    spellcheck="false"
                    role="combobox"
                    aria-expanded={kindOpen && !!kindIndex}
                    aria-controls={kindOpen && kindIndex ? LIST_ID : undefined}
                    aria-autocomplete="list"
                    aria-activedescendant={kindActiveOpt}
                    placeholder={placeholder(arg)}
                    aria-label="{node.fn} {arg.name}"
                    class:query-invalid={!!argError}
                    value={scalarValue(call.args[i])}
                    oninput={(e) => {
                      call.args[i] = e.currentTarget.value;
                      kindSel = 0;
                      kindOpen = e.currentTarget.value !== "";
                    }}
                    onkeydown={(e) => {
                      const ui = comboKey(
                        e,
                        { open: kindOpen, index: kindActiveIdx },
                        kindMatches,
                        (v) => (call.args[i] = v),
                      );
                      kindOpen = ui.open;
                      kindSel = ui.index;
                    }}
                    onblur={() => (kindOpen = false)}
                  />
                  {#if kindOpen && kindIndex}
                    <SearchDropdown
                      matches={kindMatches}
                      index={kindActiveIdx}
                      onpick={(v) => {
                        call.args[i] = v;
                        kindOpen = false;
                        kindSel = 0;
                      }}
                    />
                  {/if}
                </div>
              {:else}
                <input
                  id={fieldId(i)}
                  type="text"
                  autocomplete="off"
                  spellcheck="false"
                  list={datalist(arg)}
                  placeholder={placeholder(arg)}
                  inputmode={arg.type === "int" ? "numeric" : undefined}
                  aria-label="{node.fn} {arg.name}"
                  class:query-invalid={!!argError}
                  value={scalarValue(call.args[i])}
                  oninput={(e) => {
                    call.args[i] =
                      arg.optional && e.currentTarget.value === ""
                        ? null
                        : e.currentTarget.value;
                  }}
                />
              {/if}
              {#if argError}<p class="query-field-err">{argError}</p>{/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  </div>
{:else}
  <div
    class="query-expr-card"
    data-node-id={node.id}
    data-kind="binary"
    bind:this={wrap}
  >
    <div class="query-comp-head">{@render kindSelect()}</div>
    <div class="query-comp-body">
      <div class="query-binary-fields">
        <div class="query-expr-field">
          <span class="query-field-label">left</span>
          <QueryExpr
            bind:node={node.left}
            {errors}
            {literalEditors}
            {searchIndex}
            {attrsId}
            {kindIndex}
            context="left"
          />
        </div>
        <div class="query-expr-field">
          <span class="query-field-label">right</span>
          <QueryExpr
            bind:node={node.right}
            {errors}
            {literalEditors}
            {searchIndex}
            {attrsId}
            {kindIndex}
            context="right"
          />
        </div>
      </div>
    </div>
  </div>
{/if}
