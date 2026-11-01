<script lang="ts">
  import type { SnapshotKey } from "fuzzysort";
  import { tick } from "svelte";
  import QueryExpr from "./QueryExpr.svelte";
  import SearchInput from "./SearchInput.svelte";
  import type { Target } from "./types";
  import {
    FUNCTIONS,
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
    searchIndex,
    kindIndex,
    attrIndex,
    context = "Root",
  }: {
    node: Expr;
    errors: Map<string, string>;
    searchIndex: SnapshotKey<Target> | null;
    kindIndex: SnapshotKey<Target> | null;
    attrIndex: SnapshotKey<Target> | null;
    context?: string;
  } = $props();

  let wrap = $state<HTMLElement>();

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

  // Scalar args with a candidate population get the shared fuzzy combobox.
  function argIndex(fn: string, arg: ArgSpec): SnapshotKey<Target> | null {
    if (fn === "kind" && arg.name === "pattern") return kindIndex;
    if (arg.name === "name" || arg.name === "attr_name") return attrIndex;
    return null;
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

  function changeKind(e: Event) {
    const value = (e.currentTarget as HTMLSelectElement).value;
    if (value === kind) return;
    const next = switchKind(node, value);
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

{#if node.kind === "target"}
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
      <SearchInput
        id={fieldId("text")}
        aria-label="{context} target pattern"
        placeholder="Fuzzy :all / //pkg:target"
        invalid={!!textError}
        index={searchIndex}
        value={target.text}
        oninput={(v) => (target.text = v)}
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
              {searchIndex}
              {attrIndex}
              {kindIndex}
              context="{node.fn} {arg.name}"
            />
          {:else}
            {@const argError = fieldError(i)}
            {@const idx = argIndex(node.fn, arg)}
            <div class="query-arg">
              <label for={fieldId(i)}
                >{arg.name}{arg.optional ? " (optional)" : ""}</label
              >
              {#if idx}
                <SearchInput
                  id={fieldId(i)}
                  aria-label="{node.fn} {arg.name}"
                  placeholder={placeholder(arg)}
                  invalid={!!argError}
                  index={idx}
                  hints={[]}
                  value={scalarValue(call.args[i])}
                  oninput={(v) =>
                    (call.args[i] = arg.optional && v === "" ? null : v)}
                />
              {:else}
                <input
                  id={fieldId(i)}
                  type="text"
                  autocomplete="off"
                  spellcheck="false"
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
            {searchIndex}
            {attrIndex}
            {kindIndex}
            context="left"
          />
        </div>
        <div class="query-expr-field">
          <span class="query-field-label">right</span>
          <QueryExpr
            bind:node={node.right}
            {errors}
            {searchIndex}
            {attrIndex}
            {kindIndex}
            context="right"
          />
        </div>
      </div>
    </div>
  </div>
{/if}
