/**
 * Builder model and runner for the Bazel query editor.
 *
 * Ported from the vanilla JS in docs/mock/query-editor.html; the design lives
 * in docs/design/query-editor.md. The grammar is the mock's subset: target
 * patterns, catalog function calls, and the binary set operations
 * (`intersect`, `union`, `except`, also written `^`, `+`, `-`). `set`, `let`,
 * and `$name` variables are out of scope and rejected by the parser.
 */

/** Binary set operations accepted by the query grammar. */
export type BinOp = "intersect" | "union" | "except";

/**
 * One node of the query expression tree. `call` arguments are positional and
 * follow the function's `FnSpec.args` order: `expr` slots hold an `Expr`,
 * `word` slots a `string`, and `int` slots a `string | null` (`null` is the
 * unset optional value `serialize` drops).
 */
export type Expr =
  | { id: number; kind: "target"; text: string }
  | { id: number; kind: "call"; fn: string; args: (Expr | string | null)[] }
  | { id: number; kind: "binary"; op: BinOp; left: Expr; right: Expr };

/** One argument slot of a query function, in signature order. */
export interface ArgSpec {
  name: string;
  type: "expr" | "word" | "int";
  optional?: boolean;
  min?: number;
  default?: string;
}

/**
 * Catalog entry for one query function: the display signature, the argument
 * schema, the `expr` slot that receives the wrapped expression when the kind
 * selector switches functions (`wrapArg`), and a help sentence.
 */
export interface FnSpec {
  signature: string;
  args: ArgSpec[];
  wrapArg: number;
  help: string;
}

/** Explainer the view shows next to the `rdeps` universe argument. */
export const UNIVERSE_HELP =
  "The search scope includes the targets in universe and their transitive dependencies. It does not restrict results to a package.";

/**
 * The supported `bazel query` functions, in catalog order. Display,
 * validation, and serialization all read from this one table.
 */
export const FUNCTIONS: Record<string, FnSpec> = {
  deps: {
    signature: "deps(expr[, depth])",
    args: [
      { name: "expr", type: "expr" },
      { name: "depth", type: "int", optional: true, min: 0 },
    ],
    wrapArg: 0,
    help: "Returns the transitive dependencies of expr, including expr itself.",
  },
  rdeps: {
    signature: "rdeps(universe, expr[, depth])",
    args: [
      { name: "universe", type: "expr", default: "//..." },
      { name: "expr", type: "expr" },
      { name: "depth", type: "int", optional: true, min: 0 },
    ],
    wrapArg: 1,
    help: "Finds reverse dependencies of expr within universe and its transitive dependencies.",
  },
  somepath: {
    signature: "somepath(from, to)",
    args: [
      { name: "from", type: "expr" },
      { name: "to", type: "expr" },
    ],
    wrapArg: 0,
    help: "Finds one dependency path between from and to.",
  },
  allpaths: {
    signature: "allpaths(from, to)",
    args: [
      { name: "from", type: "expr" },
      { name: "to", type: "expr" },
    ],
    wrapArg: 0,
    help: "Returns all targets on dependency paths between from and to.",
  },
  kind: {
    signature: "kind(pattern, expr)",
    args: [
      { name: "pattern", type: "word", default: ".*_test rule" },
      { name: "expr", type: "expr" },
    ],
    wrapArg: 1,
    help: "Filters targets by a regular expression on target kind.",
  },
  filter: {
    signature: "filter(pattern, expr)",
    args: [
      { name: "pattern", type: "word", default: "//app/" },
      { name: "expr", type: "expr" },
    ],
    wrapArg: 1,
    help: "Filters targets by a regular expression on their labels.",
  },
  attr: {
    signature: "attr(name, pattern, expr)",
    args: [
      { name: "name", type: "word", default: "size" },
      { name: "pattern", type: "word", default: "^small$" },
      { name: "expr", type: "expr" },
    ],
    wrapArg: 2,
    help: "Filters attribute values by a regular expression. List attributes such as tags are matched as strings in [value, ...] form.",
  },
  labels: {
    signature: "labels(attr_name, expr)",
    args: [
      { name: "attr_name", type: "word", default: "srcs" },
      { name: "expr", type: "expr" },
    ],
    wrapArg: 1,
    help: "Returns targets referenced by a label-valued attribute such as srcs, deps, or data.",
  },
  tests: {
    signature: "tests(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
    help: "Selects test rules and expands test_suite targets into individual tests, applying the suite's tags and size filters.",
  },
  buildfiles: {
    signature: "buildfiles(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
    help: "Returns BUILD files defining the targets' packages and their loaded .bzl files.",
  },
  loadfiles: {
    signature: "loadfiles(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
    help: "Returns .bzl files loaded by the packages of expr.",
  },
  executables: {
    signature: "executables(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
    help: "Returns executable, non-test rule targets in expr.",
  },
  siblings: {
    signature: "siblings(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
    help: "Returns targets in the same packages as expr.",
  },
  same_pkg_direct_rdeps: {
    signature: "same_pkg_direct_rdeps(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
    help: "Returns targets in the same packages that directly depend on expr.",
  },
  some: {
    signature: "some(expr[, count])",
    args: [
      { name: "expr", type: "expr" },
      { name: "count", type: "int", optional: true, min: 1 },
    ],
    wrapArg: 0,
    help: "Returns up to count arbitrarily chosen targets from expr.",
  },
  visible: {
    signature: "visible(predicate, input)",
    args: [
      { name: "predicate", type: "expr" },
      { name: "input", type: "expr" },
    ],
    wrapArg: 1,
    help: "Returns targets in input that are visible to every target in predicate.",
  },
};

let nextNodeId = 1;

/**
 * Creates a target-pattern node. Empty text is the expression hole a fresh
 * slot starts as; the view decides whether it renders as a hole or a literal
 * input via its literalEditors set (see `isExprHole`).
 */
export function target(text = ""): Expr {
  return { id: nextNodeId++, kind: "target", text };
}

/**
 * Creates a call node for `fn` with schema defaults: `expr` args become empty
 * targets (or their `default` text), `int` args start as `null`, and `word`
 * args take their `default`. `keep`, when given, fills the `wrapArg` slot —
 * the expression a kind switch carries over.
 */
export function makeCall(fn: string, keep?: Expr): Expr {
  const schema = FUNCTIONS[fn];
  const args: (Expr | string | null)[] = schema.args.map((arg) => {
    if (arg.type === "expr") return target(arg.default ?? "");
    if (arg.type === "int") return null;
    return arg.default ?? "";
  });
  const n = { id: nextNodeId++, kind: "call" as const, fn, args };
  if (keep !== undefined) n.args[schema.wrapArg] = keep;
  return n;
}

/**
 * Creates the default node for a kind-selector value: an empty target for
 * "target", a call with schema defaults for a function name, or a binary node
 * with two empty targets for a set operation.
 */
export function defaultNodeFor(kind: string): Expr {
  if (kind === "target") return target("");
  if (Object.hasOwn(FUNCTIONS, kind)) return makeCall(kind);
  return {
    id: nextNodeId++,
    kind: "binary",
    op: kind as BinOp,
    left: target(""),
    right: target(""),
  };
}

/**
 * Kind-selector groups, in display order: the bare target pattern, every
 * catalog function, then the three set operations.
 */
export const KIND_GROUPS: ReadonlyArray<
  readonly [string, ReadonlyArray<readonly [string, string]>]
> = [
  ["Expressions", [["target", "Target pattern"]]],
  ["Functions", Object.keys(FUNCTIONS).map((f) => [f, f] as const)],
  [
    "Set operations",
    [
      ["intersect", "intersect"],
      ["union", "union"],
      ["except", "except"],
    ],
  ],
];

const KEYWORDS = new Set(["except", "in", "intersect", "let", "set", "union"]);

function quoteWord(value: string, force = false): string {
  // `$` is excluded everywhere: a bare `$name` is a let-variable reference to
  // bazel, which the Builder does not support.
  const safe =
    /^[A-Za-z0-9_./:@~*[\]-]+$/.test(value) && !/^[-*]/.test(value) && !KEYWORDS.has(value);
  if (!force && safe) return value;
  if (!value.includes('"')) return '"' + value + '"';
  if (!value.includes("'")) return "'" + value + "'";
  throw new Error(
    "The word contains both quote types. Bazel query strings do not support escaping.",
  );
}

function serializeNode(n: Expr): string {
  switch (n.kind) {
    case "target":
      return quoteWord(n.text);
    case "binary":
      return `(${serializeNode(n.left)} ${n.op} ${serializeNode(n.right)})`;
    case "call": {
      const schema = FUNCTIONS[n.fn];
      const args = schema.args.flatMap((arg, i) => {
        const value = n.args[i];
        if (arg.optional && (value === null || value === "")) return [];
        if (arg.type === "expr") return [serializeNode(value as Expr)];
        if (arg.type === "int") return [String(value).trim()];
        return [quoteWord(String(value), arg.name === "pattern")];
      });
      return `${n.fn}(${args.join(", ")})`;
    }
  }
}

/**
 * Turns an expression tree back into query text: binary nodes are always
 * parenthesized, words are quoted per `quoteWord`, and unset optional
 * arguments are dropped. A word containing both quote types cannot be
 * represented and becomes `{ ok: false }` — Bazel query has no escaping.
 */
export function serialize(root: Expr): { ok: true; text: string } | { ok: false; error: string } {
  try {
    return { ok: true, text: serializeNode(root) };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

function shellQuote(value: string): string {
  return "'" + value.replaceAll("'", "'\\''") + "'";
}

/** The two dependency flags the editor offers next to the generated query. */
export interface CommandFlags {
  noImplicit: boolean;
  noTool: boolean;
}

/**
 * Mock defaults: implicit deps are excluded (unlike Bazel's default), tool
 * deps are kept. Preset loading resets the flags to these values.
 */
export const DEFAULT_FLAGS: CommandFlags = { noImplicit: true, noTool: false };

/**
 * The `bazel query '<expr>' --output=<fmt> ...` line the view displays and
 * Copy command puts on the clipboard. It mirrors the argv `run_query`
 * executes so a pasted command produces the displayed result. Execution
 * never goes through a shell.
 */
export function commandLine(
  exprText: string,
  output: "raw" | "graph",
  flags: CommandFlags,
): string {
  return `bazel query ${shellQuote(exprText)} --output=${
    output === "graph" ? "graph --nograph:factored" : "label"
  }${flags.noImplicit ? " --noimplicit_deps" : ""}${flags.noTool ? " --notool_deps" : ""}`;
}

/**
 * Arguments for the `run_query` invoke. `output` is the bazel `--output`
 * format the core accepts: the Raw view runs `label`, the Graph view `graph`.
 */
export function queryArgv(
  expr: string,
  output: "raw" | "graph",
  flags: CommandFlags,
): {
  expr: string;
  output: string;
  noImplicitDeps: boolean;
  noToolDeps: boolean;
} {
  return {
    expr,
    output: output === "graph" ? "graph" : "label",
    noImplicitDeps: flags.noImplicit,
    noToolDeps: flags.noTool,
  };
}

/**
 * Identity of one stored result: the flag bits, the result view, and the
 * expression text — the mock's `currentKey` with the view folded in, since
 * each view keeps its own result. Comparing `keyFor` of the current inputs
 * against a stored key decides whether that result is stale.
 */
export function keyFor(expr: string, view: "raw" | "graph", flags: CommandFlags): string {
  return `${flags.noImplicit ? "1" : "0"}${flags.noTool ? "1" : "0"}|${view}|${expr}`;
}

interface Token {
  type: string;
  value: string;
  pos: number;
  /** Quoted words keep their literal text: `"$x"` is a label, not a variable. */
  quoted?: boolean;
}

function syntaxError(t: { pos: number }, msg: string): Error & { pos: number } {
  const e = new Error(`At position ${t.pos}: ${msg}`) as Error & {
    pos: number;
  };
  e.pos = t.pos;
  return e;
}

const WORD_CHARS = /[A-Za-z0-9*/@.\-_:$~[\]]/;

function tokenize(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (/\s/.test(c)) {
      i++;
      continue;
    }
    if ("(),^+-".includes(c)) {
      tokens.push({ type: c, value: c, pos: i });
      i++;
      continue;
    }
    if (c === '"' || c === "'") {
      const start = i;
      i++;
      let v = "";
      while (i < text.length && text[i] !== c) {
        v += text[i];
        i++;
      }
      if (i >= text.length) throw syntaxError({ pos: text.length }, "Unterminated quoted word");
      i++;
      tokens.push({ type: "word", value: v, pos: start, quoted: true });
      continue;
    }
    const start = i;
    let v = "";
    while (i < text.length) {
      const ch = text[i];
      if (ch === "+" && v.startsWith("@@")) {
        v += ch;
        i++;
        continue;
      }
      if (WORD_CHARS.test(ch) || ch.charCodeAt(0) > 127) {
        v += ch;
        i++;
        continue;
      }
      break;
    }
    if (v === "") throw syntaxError({ pos: i }, `'${c}' must be quoted when used in a target name`);
    tokens.push({
      type: KEYWORDS.has(v) ? v : "word",
      value: v,
      pos: start,
    });
  }
  tokens.push({ type: "eof", value: "", pos: text.length });
  return tokens;
}

function parseQuery(text: string): Expr {
  const tokens = tokenize(text);
  let index = 0;
  const peek = () => tokens[index];
  const take = (type: string) => {
    const t = peek();
    if (t.type !== type) throw syntaxError(t, `expected ${type}`);
    index++;
    return t;
  };
  const takeWord = () => {
    const t = take("word");
    if (!t.quoted && t.value.startsWith("$"))
      throw syntaxError(t, "variables are not supported in Builder");
    return t.value;
  };
  const OPS: Record<string, BinOp> = {
    intersect: "intersect",
    "^": "intersect",
    union: "union",
    "+": "union",
    except: "except",
    "-": "except",
  };
  function expression(): Expr {
    let left = primary();
    while (Object.hasOwn(OPS, peek().type)) {
      const op = OPS[tokens[index++].type];
      left = { id: nextNodeId++, kind: "binary", op, left, right: primary() };
    }
    return left;
  }
  function primary(): Expr {
    const t = peek();
    if (t.type === "(") {
      take("(");
      const value = expression();
      take(")");
      return value;
    }
    const word = takeWord();
    if (peek().type === "(") {
      if (!Object.hasOwn(FUNCTIONS, word))
        throw syntaxError(t, `${word} is not supported by this Builder`);
      take("(");
      const schema = FUNCTIONS[word];
      const args: (Expr | string | null)[] = [];
      for (let i = 0; i < schema.args.length; i++) {
        const arg = schema.args[i];
        if (peek().type === ")" && arg.optional) {
          args.push(null);
          continue;
        }
        if (i > 0) take(",");
        args.push(arg.type === "expr" ? expression() : takeWord());
      }
      take(")");
      return { id: nextNodeId++, kind: "call", fn: word, args };
    }
    return target(word);
  }
  const root = expression();
  take("eof");
  return root;
}

/**
 * Parses query text into an expression tree. On failure `error` carries the
 * mock's "At position …" message and `pos` the character offset. Keywords
 * (`set`, `let`, …), `$name` variables, and functions outside `FUNCTIONS` are
 * rejected — such queries stay usable in Code mode but cannot enter Builder.
 */
export function parse(
  text: string,
): { ok: true; expr: Expr } | { ok: false; error: string; pos?: number } {
  try {
    return { ok: true, expr: parseQuery(text) };
  } catch (e) {
    const error = e as Error & { pos?: number };
    const out: { ok: false; error: string; pos?: number } = {
      ok: false,
      error: error instanceof Error ? error.message : String(e),
    };
    if (typeof error.pos === "number") out.pos = error.pos;
    return out;
  }
}

function wordQuotable(s: string): boolean {
  return !(s.includes('"') && s.includes("'"));
}

/**
 * Per-field errors for an expression tree, keyed like the mock's `data-err`
 * attributes: `${nodeId}:text` for target patterns and `${nodeId}:${index}`
 * for scalar call arguments.
 */
export function validate(root: Expr): Map<string, string> {
  const errors: { id: number; arg: string | number; msg: string }[] = [];
  function walk(n: Expr) {
    switch (n.kind) {
      case "target": {
        const t = n.text;
        if (t.trim() === "") {
          errors.push({ id: n.id, arg: "text", msg: "Required" });
        } else if (/^[-*]/.test(t)) {
          errors.push({
            id: n.id,
            arg: "text",
            msg: "Targets cannot start with - or *. Use //pkg:target syntax.",
          });
          // oxlint-disable-next-line no-control-regex -- control characters are the thing being rejected
        } else if (/[\x00-\x1f\x7f-\x9f]/.test(t)) {
          errors.push({
            id: n.id,
            arg: "text",
            msg: "Control characters are not allowed",
          });
        } else if (!wordQuotable(t)) {
          errors.push({
            id: n.id,
            arg: "text",
            msg: "The word contains both quote types",
          });
        }
        break;
      }
      case "binary":
        walk(n.left);
        walk(n.right);
        break;
      case "call": {
        const schema = FUNCTIONS[n.fn];
        schema.args.forEach((arg, i) => {
          const v = n.args[i];
          if (arg.type === "expr") {
            walk(v as Expr);
            return;
          }
          if (arg.type === "int") {
            if (v === null || v === "") {
              if (!arg.optional) errors.push({ id: n.id, arg: i, msg: "Required" });
            } else {
              const s = String(v).trim();
              if (!/^(0|[0-9]+)$/.test(s))
                errors.push({
                  id: n.id,
                  arg: i,
                  msg: "Enter a non-negative integer",
                });
              else if (!Number.isSafeInteger(+s) || +s > 2147483647)
                errors.push({
                  id: n.id,
                  arg: i,
                  msg: "Must be at most 2147483647",
                });
              else if (arg.min != null && +s < arg.min)
                errors.push({
                  id: n.id,
                  arg: i,
                  msg: `Must be at least ${arg.min}`,
                });
            }
            return;
          }
          const s = String(v);
          // oxlint-disable-next-line no-control-regex -- control characters are the thing being rejected
          if (/[\x00-\x1f\x7f-\x9f]/.test(s))
            errors.push({
              id: n.id,
              arg: i,
              msg: "Control characters are not allowed",
            });
          else if (!wordQuotable(s))
            errors.push({
              id: n.id,
              arg: i,
              msg: "The word contains both quote types",
            });
        });
        break;
      }
    }
  }
  walk(root);
  return new Map(errors.map((e) => [`${e.id}:${e.arg}`, e.msg]));
}

type CallNode = Extract<Expr, { kind: "call" }>;

function switchedFunction(current: CallNode, fn: string): CallNode {
  const old = FUNCTIONS[current.fn];
  const nextSchema = FUNCTIONS[fn];
  const next = makeCall(fn, current.args[old.wrapArg] as Expr);
  if (next.kind !== "call") throw new Error("unreachable: makeCall builds call nodes");
  next.id = current.id;
  nextSchema.args.forEach((arg, i) => {
    if (i === nextSchema.wrapArg) return;
    const j = old.args.findIndex(
      (a, j) => j !== old.wrapArg && a.name === arg.name && a.type === arg.type,
    );
    if (j >= 0) next.args[i] = current.args[j];
  });
  return next;
}

/** Kind-selector value for a node: the function name, the set op, or "target". */
export function kindOf(n: Expr): string {
  if (n.kind === "call") return n.fn;
  if (n.kind === "binary") return n.op;
  return n.kind;
}

/**
 * Re-kinds a node for the kind selector. Function → function moves the old
 * `wrapArg` expression into the new `wrapArg` slot and carries over other
 * arguments of the same name and type, keeping the node's id. Binary → binary
 * keeps both children. Anything else replaces the node with
 * `defaultNodeFor(kind)` — a fresh id and no carry-over.
 */
export function switchKind(node: Expr, kind: string): Expr {
  if (kind === kindOf(node)) return node;
  if (node.kind === "call" && Object.hasOwn(FUNCTIONS, kind)) return switchedFunction(node, kind);
  if (node.kind === "binary" && (kind === "intersect" || kind === "union" || kind === "except")) {
    return {
      id: node.id,
      kind: "binary",
      op: kind,
      left: node.left,
      right: node.right,
    };
  }
  return defaultNodeFor(kind);
}

/**
 * Whether a node renders as an unfilled expression slot: an empty target the
 * view has not marked as a literal text input (its `literalEditors` set).
 */
export function isExprHole(n: Expr, literalEditors: ReadonlySet<number>): boolean {
  return n.kind === "target" && n.text === "" && !literalEditors.has(n.id);
}

/**
 * Whether a Code-mode draft differs from the Builder expression, mirroring
 * the mock's diverged check: trimmed draft vs. trimmed serialization, with an
 * unserializable tree comparing as the empty string.
 */
export function codeDiverged(root: Expr, draft: string): boolean {
  const s = serialize(root);
  return draft.trim() !== (s.ok ? s.text.trim() : "");
}

const PRESETS: Record<string, { expr: string; desc?: string }> = {
  commonDeps: {
    expr: "(deps(//app:server) intersect deps(//tools:cli))",
    desc: "Shared dependencies",
  },
  rdepsKind: { expr: 'kind(".*_test rule", rdeps(//..., //app/lib:parser))' },
  deps: { expr: "deps(//app:server)" },
  somepath: { expr: "somepath(//app:server, //app/lib:parser)" },
  onlyServer: {
    expr: "(deps(//app:server) except deps(//tools:cli))",
    desc: "Dependencies unique to one target",
  },
  smallTestSources: {
    expr: 'labels(srcs, attr(size, "^small$", //app/...))',
    desc: "Filter by attribute, then get srcs",
  },
  suiteTests: { expr: "tests(//app:all_tests)", desc: "Expand test suites" },
  rdepsTests: { expr: "tests(rdeps(//..., //app/lib:parser))" },
  allpaths: { expr: "allpaths(//app:server, //app/lib:parser)" },
  buildfiles: { expr: "buildfiles(deps(//app:server))" },
};

const EXAMPLE_LABELS: Record<string, string> = {
  commonDeps: "deps ∩ deps",
  rdepsKind: "rdeps + kind",
  deps: "deps",
  somepath: "somepath",
  onlyServer: "deps − deps",
  smallTestSources: "attr → labels",
  suiteTests: "tests",
  rdepsTests: "rdeps → tests",
  allpaths: "allpaths",
  buildfiles: "buildfiles",
};

/**
 * Sidebar examples in the mock's order; `build` returns a fresh AST per call.
 * `name` is the display label from the mock; `id` is the stable key.
 */
export const EXAMPLES: ReadonlyArray<{
  id: string;
  name: string;
  desc?: string;
  build: () => Expr;
}> = Object.keys(PRESETS).map((id) => ({
  id,
  name: EXAMPLE_LABELS[id],
  desc: PRESETS[id].desc,
  build: () => parseQuery(PRESETS[id].expr),
}));

/**
 * Builds the named example expression, or null for an unknown name. Loading
 * an example replaces the current expression, resets the flags to
 * `DEFAULT_FLAGS`, and returns to Builder mode — the view performs those
 * resets itself and confirms first when `codeDiverged` says a Code draft
 * would be lost; this layer never confirms.
 */
export function applyExample(id: string): Expr | null {
  return EXAMPLES.find((e) => e.id === id)?.build() ?? null;
}

/**
 * One `bazel query` invocation's outcome as the core serializes it
 * (camelCase). A non-zero `exitCode` is a query failure, not an IPC error:
 * the reply still lands here and the view renders `exitCode` + `stderrTail`.
 */
export interface QueryOutput {
  exitCode: number | null;
  stdout: string;
  truncated: boolean;
  stderrTail: string;
  timedOut: boolean;
  stopped: boolean;
}

/** IPC boundary the query runner uses to call the core. */
export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
}

/** A finished run's output bound to the `keyFor` key it ran under. */
export interface Stored extends QueryOutput {
  key: string;
}

/**
 * Execution state pushed to the view on every change. The view compares each
 * stored result's `key` against `keyFor` of the current inputs for staleness.
 * The controller only tracks its own in-flight run — the view owns the global
 * busy picture and combines both.
 */
export interface QueryState {
  running: boolean;
  stopping: boolean;
  error: string | null;
  results: { raw?: Stored; graph?: Stored };
}

/** The state before any run: nothing in flight, no results. */
export function emptyQueryState(): QueryState {
  return { running: false, stopping: false, error: null, results: {} };
}

/**
 * Creates a query runner scoped to one editor. `run` invokes `run_query` with
 * the `queryArgv` arguments and stores the reply under the chosen view keyed
 * by `keyFor`; a second `run` while one is in flight is ignored — the core
 * serializes bazel work, and the view decides when to call again. Errors
 * thrown by `invoke` (busy/spawn class) land in `error`; a non-zero exitCode
 * is data, not an error. `stop` invokes `stop_query`. Replies arriving after
 * `dispose()` or after being superseded are dropped via a generation counter;
 * a run still in flight at `dispose()` is also stopped — its result would be
 * dropped anyway, and the slot should not stay held by a discarded query.
 */
export function createQuery(
  t: Transport,
  changed: (s: QueryState) => void,
): {
  run(expr: string, view: "raw" | "graph", flags: CommandFlags): Promise<void>;
  stop(): Promise<void>;
  dispose(): void;
} {
  let state = emptyQueryState();
  let generation = 0;
  let disposed = false;
  function update(patch: Partial<QueryState>) {
    if (disposed) return;
    state = { ...state, ...patch };
    changed(state);
  }
  async function run(expr: string, view: "raw" | "graph", flags: CommandFlags): Promise<void> {
    if (disposed || state.running) return;
    const request = ++generation;
    const key = keyFor(expr, view, flags);
    update({ running: true, stopping: false, error: null });
    try {
      const output = await t.invoke<QueryOutput>("run_query", queryArgv(expr, view, flags));
      if (disposed || request !== generation) return;
      const stored: Stored = { ...output, key };
      const results = { ...state.results };
      results[view] = stored;
      update({ running: false, stopping: false, results });
    } catch (error) {
      if (disposed || request !== generation) return;
      update({ running: false, stopping: false, error: String(error) });
    }
  }
  async function stop(): Promise<void> {
    if (disposed || !state.running || state.stopping) return;
    update({ stopping: true });
    try {
      await t.invoke("stop_query");
    } catch (error) {
      if (!disposed) update({ stopping: false, error: String(error) });
    }
  }
  function dispose() {
    disposed = true;
    ++generation;
    if (state.running) void t.invoke("stop_query").catch(() => {});
  }
  return { run, stop, dispose };
}
