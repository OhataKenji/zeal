<script lang="ts">
  import { tick } from "svelte";
  import QueryExpr from "./QueryExpr.svelte";
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
    labelsId,
    attrsId,
    kindsId,
    context = "Root",
  }: {
    node: Expr;
    errors: Map<string, string>;
    literalEditors: Set<number>;
    labelsId: string;
    attrsId: string;
    kindsId: string;
    context?: string;
  } = $props();

  let wrap = $state<HTMLElement>();

  // An empty target is an unfilled expression slot until the user picks
  // "Target pattern" — the mark lives in `literalEditors`, not in the Expr.
  const hole = $derived(isExprHole(node, literalEditors));
  const kind = $derived(kindOf(node));
  const textError = $derived(errors.get(`${node.id}:text`));

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

  function datalist(fn: string, arg: ArgSpec): string | undefined {
    if (arg.name === "name" || arg.name === "attr_name") return attrsId;
    if (arg.name === "pattern" && fn === "kind") return kindsId;
    return undefined;
  }

  function placeholder(arg: ArgSpec): string {
    if (arg.optional) return "optional";
    if (arg.type === "int") return "0";
    return arg.name === "pattern" ? "regex" : "word";
  }

  function argIncluded(spec: ArgSpec, value: Expr | string | null): boolean {
    return (
      !spec.optional || (value !== null && value !== undefined && value !== "")
    );
  }

  // A comma separates two rendered argument slots; trailing optional slots that
  // are unset get no comma, matching the mock's updateArgumentSeparators.
  function showComma(specs: ArgSpec[], values: (Expr | string | null)[], i: number) {
    return (
      argIncluded(specs[i], values[i]) &&
      specs.some((_, j) => j > i && argIncluded(specs[j], values[j]))
    );
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
  <div
    class="query-atom-row"
    data-node-id={node.id}
    data-kind="target"
    bind:this={wrap}
  >
    {@render kindSelect()}
    <div class="query-arg">
      <label for={fieldId("text")}>Target pattern</label>
      <input
        id={fieldId("text")}
        type="text"
        autocomplete="off"
        spellcheck="false"
        list={labelsId}
        placeholder="//pkg:target or //..."
        aria-label="{context} target pattern"
        class:query-invalid={!!textError}
        bind:value={node.text}
      />
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
              {labelsId}
              {attrsId}
              {kindsId}
              context="{node.fn} {arg.name}"
            />
          {:else}
            {@const argError = fieldError(i)}
            <div class="query-arg">
              <label for={fieldId(i)}
                >{arg.name}{arg.optional ? " (optional)" : ""}</label
              >
              <input
                id={fieldId(i)}
                type="text"
                autocomplete="off"
                spellcheck="false"
                list={datalist(node.fn, arg)}
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
              {#if argError}<p class="query-field-err">{argError}</p>{/if}
            </div>
          {/if}
          {#if showComma(schema.args, call.args, i)}
            <span class="query-arg-comma">,</span>
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
            {labelsId}
            {attrsId}
            {kindsId}
            context="left"
          />
        </div>
        <div class="query-expr-field">
          <span class="query-field-label">right</span>
          <QueryExpr
            bind:node={node.right}
            {errors}
            {literalEditors}
            {labelsId}
            {attrsId}
            {kindsId}
            context="right"
          />
        </div>
      </div>
    </div>
  </div>
{/if}
