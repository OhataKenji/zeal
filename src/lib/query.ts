import { COMMANDS } from "./commands.ts";
import { labelTarget } from "./search.ts";
import type { Target } from "./types.ts";

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
 * schema, and the `expr` slot that receives the wrapped expression when the kind
 * selector switches functions (`wrapArg`).
 */
export interface FnSpec {
  signature: string;
  args: ArgSpec[];
  wrapArg: number;
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
  },
  rdeps: {
    signature: "rdeps(universe, expr[, depth])",
    args: [
      { name: "universe", type: "expr", default: "//..." },
      { name: "expr", type: "expr" },
      { name: "depth", type: "int", optional: true, min: 0 },
    ],
    wrapArg: 1,
  },
  somepath: {
    signature: "somepath(from, to)",
    args: [
      { name: "from", type: "expr" },
      { name: "to", type: "expr" },
    ],
    wrapArg: 0,
  },
  allpaths: {
    signature: "allpaths(from, to)",
    args: [
      { name: "from", type: "expr" },
      { name: "to", type: "expr" },
    ],
    wrapArg: 0,
  },
  kind: {
    signature: "kind(pattern, expr)",
    args: [
      { name: "pattern", type: "word", default: ".*_test rule" },
      { name: "expr", type: "expr" },
    ],
    wrapArg: 1,
  },
  filter: {
    signature: "filter(pattern, expr)",
    args: [
      { name: "pattern", type: "word", default: "//app/" },
      { name: "expr", type: "expr" },
    ],
    wrapArg: 1,
  },
  attr: {
    signature: "attr(name, pattern, expr)",
    args: [
      { name: "name", type: "word", default: "size" },
      { name: "pattern", type: "word", default: "^small$" },
      { name: "expr", type: "expr" },
    ],
    wrapArg: 2,
  },
  labels: {
    signature: "labels(attr_name, expr)",
    args: [
      { name: "attr_name", type: "word", default: "srcs" },
      { name: "expr", type: "expr" },
    ],
    wrapArg: 1,
  },
  tests: {
    signature: "tests(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
  },
  buildfiles: {
    signature: "buildfiles(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
  },
  loadfiles: {
    signature: "loadfiles(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
  },
  executables: {
    signature: "executables(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
  },
  siblings: {
    signature: "siblings(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
  },
  same_pkg_direct_rdeps: {
    signature: "same_pkg_direct_rdeps(expr)",
    args: [{ name: "expr", type: "expr" }],
    wrapArg: 0,
  },
  some: {
    signature: "some(expr[, count])",
    args: [
      { name: "expr", type: "expr" },
      { name: "count", type: "int", optional: true, min: 1 },
    ],
    wrapArg: 0,
  },
  visible: {
    signature: "visible(predicate, input)",
    args: [
      { name: "predicate", type: "expr" },
      { name: "input", type: "expr" },
    ],
    wrapArg: 1,
  },
};

let nextNodeId = 1;

/** Creates a target-pattern node; a new slot starts with empty text. */
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
  return `bazel --noblock_for_lock query ${shellQuote(exprText)} --output=${
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

function readWord(text: string, start: number): [token: Token, end: number] {
  const quote = text[start];
  if (quote === '"' || quote === "'") {
    const end = text.indexOf(quote, start + 1);
    if (end < 0) throw syntaxError({ pos: text.length }, "Unterminated quoted word");
    return [{ type: "word", value: text.slice(start + 1, end), pos: start, quoted: true }, end + 1];
  }
  let end = start;
  while (
    end < text.length &&
    (WORD_CHARS.test(text[end]) ||
      text.charCodeAt(end) > 127 ||
      (text[end] === "+" && text.startsWith("@@", start)))
  ) {
    end++;
  }
  if (end === start)
    throw syntaxError({ pos: start }, `'${text[start]}' must be quoted when used in a target name`);
  const value = text.slice(start, end);
  return [{ type: KEYWORDS.has(value) ? value : "word", value, pos: start }, end];
}

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
    const [token, end] = readWord(text, i);
    tokens.push(token);
    i = end;
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
    if (peek().type !== "(") return target(word);
    if (!Object.hasOwn(FUNCTIONS, word))
      throw syntaxError(t, `${word} is not supported by this Builder`);
    const schema = FUNCTIONS[word];
    const args = callArgs(schema);
    return { id: nextNodeId++, kind: "call", fn: word, args };
  }
  function callArgs(schema: FnSpec): (Expr | string | null)[] {
    take("(");
    const args: (Expr | string | null)[] = [];
    for (let i = 0; i < schema.args.length; i++) {
      args.push(callArg(schema.args[i], i));
    }
    take(")");
    return args;
  }
  function callArg(arg: ArgSpec, i: number): Expr | string | null {
    if (peek().type === ")" && arg.optional) return null;
    if (i > 0) take(",");
    return arg.type === "expr" ? expression() : takeWord();
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

function integerError(value: unknown, arg: ArgSpec): string | null {
  if (value === null || value === "") return arg.optional ? null : "Required";
  const s = String(value).trim();
  if (!/^(0|[0-9]+)$/.test(s)) return "Enter a non-negative integer";
  if (!Number.isSafeInteger(+s) || +s > 2147483647) return "Must be at most 2147483647";
  if (arg.min != null && +s < arg.min) return `Must be at least ${arg.min}`;
  return null;
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
            const msg = integerError(v, arg);
            if (msg) errors.push({ id: n.id, arg: i, msg });
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
 * Whether a Code-mode draft differs from the Builder expression, mirroring
 * the mock's diverged check: trimmed draft vs. trimmed serialization, with an
 * unserializable tree comparing as the empty string.
 */
export function codeDiverged(root: Expr, draft: string): boolean {
  const s = serialize(root);
  return draft.trim() !== (s.ok ? s.text.trim() : "");
}

/**
 * Candidate list for kind()'s pattern argument: the wildcard-test hint, then
 * the non-rule kinds (`source file`, `generated file`) the snapshot cannot
 * list because it only indexes rules, then every rule class in the snapshot.
 * Rule classes take the ` rule` suffix kind patterns expect, except classes
 * already ending in "file".
 */
export function kindCandidates(targets: Target[]): Target[] {
  return [
    ".*_test rule",
    "source file",
    "generated file",
    ...[...new Set(targets.map((t) => t.ruleClass))]
      .sort()
      .map((rc) => (rc.endsWith("file") ? rc : `${rc} rule`)),
  ].map((label) => labelTarget(label, "kind"));
}

/**
 * Candidate list for attr()'s `name` and labels()'s `attr_name` arguments.
 * The fast label_kind snapshot carries no attributes, so — like the function
 * catalog — this is a hand-maintained hint list of common Bazel attribute
 * names, label-typed and scalar alike. Completion only assists; any name the
 * list lacks can still be typed.
 */
export const ATTR_NAMES: readonly string[] = [
  "actual",
  "alwayslink",
  "args",
  "cmd",
  "compatible_with",
  "constraints",
  "copts",
  "data",
  "deps",
  "env",
  "exec_compatible_with",
  "exec_properties",
  "exec_tools",
  "exports",
  "features",
  "flaky",
  "hdrs",
  "includes",
  "instrumented_files",
  "javacopts",
  "jvm_flags",
  "licenses",
  "linkopts",
  "linkstatic",
  "main_class",
  "outs",
  "plugins",
  "resources",
  "runtime_deps",
  "shard_count",
  "size",
  "srcs",
  "stamp",
  "suites",
  "tags",
  "target_compatible_with",
  "testonly",
  "tests",
  "timeout",
  "toolchains",
  "tools",
  "visibility",
];

/** ATTR_NAMES as search candidates, labeled "attribute" in the dropdown. */
export function attrCandidates(): Target[] {
  return ATTR_NAMES.map((label) => labelTarget(label, "attribute"));
}

/**
 * Synthetic pattern candidates for target inputs: the workspace-wide `//...`,
 * plus `//pkg/...` and `//pkg:all` for every package the index lists. Queries
 * often take a package or subtree rather than one target (`tests(//app/...)`,
 * the rdeps universe), and these patterns are not labels the index has.
 */
export function packagePatterns(targets: Target[]): Target[] {
  const out: Target[] = [labelTarget("//...", "package pattern")];
  for (const pkg of [...new Set(targets.map((t) => t.pkg))].sort()) {
    if (pkg === "") {
      out.push(labelTarget("//:all", "package pattern"));
      continue;
    }
    out.push(
      labelTarget(`//${pkg}/...`, "package pattern"),
      labelTarget(`//${pkg}:all`, "package pattern"),
    );
  }
  return out;
}

// Ordered like the bazel query guide: single-function queries first (what X
// needs, how it connects, who needs X), then existence enumeration, then the
// composite idioms (diffs) and plumbing.
//
// Every example only names @@platforms constraint-graph targets: canonical
// `@@` names resolve without a use_repo declaration, so each runs verbatim in
// any repository (even an empty MODULE.bazel), and a ~60-target universe
// keeps them cheap in huge monorepos. tests() has no universal example —
// no test_suite is guaranteed to exist.
const PRESETS: Record<string, { expr: string; desc: string }> = {
  deps: {
    expr: "deps(@@platforms//os:all)",
    desc: "Transitive dependencies",
  },
  somepath: {
    expr: "somepath(@@platforms//os:all, @@platforms//os:os)",
    desc: "One path between two targets",
  },
  allpaths: {
    expr: "allpaths(@@platforms//os:all, @@platforms//os:os)",
    desc: "Every path between two targets",
  },
  rdeps: {
    expr: "rdeps(@@platforms//..., @@platforms//os:os)",
    desc: "Reverse dependencies",
  },
  kindConstraint: {
    expr: 'kind("constraint_value rule", @@platforms//...)',
    desc: "Find targets by rule kind",
  },
  siblings: {
    expr: "siblings(@@platforms//os:linux)",
    desc: "Targets in the same package",
  },
  attrLabels: {
    expr: 'labels(constraint_setting, attr(constraint_setting, "cpu", @@platforms//...))',
    desc: "Filter by attribute, then get labels",
  },
  osExceptKind: {
    expr: '(@@platforms//os:all) except kind("constraint_value rule", @@platforms//os:all)',
    desc: "Package targets minus a rule kind",
  },
  depsIntersectKind: {
    expr: '(deps(@@platforms//os:all) intersect kind("constraint_value rule", @@platforms//os:all))',
    desc: "Constraint values inside the deps",
  },
  buildfiles: {
    expr: "buildfiles(deps(@@platforms//os:all))",
    desc: "BUILD files in the deps",
  },
};

const EXAMPLE_LABELS: Record<string, string> = {
  deps: "deps",
  somepath: "somepath",
  allpaths: "allpaths",
  rdeps: "rdeps",
  kindConstraint: "kind constraint_value",
  siblings: "siblings",
  attrLabels: "attr → labels",
  osExceptKind: "all − kind",
  depsIntersectKind: "deps ∩ kind",
  buildfiles: "buildfiles",
};

/**
 * Sidebar examples in the mock's order; `build` returns a fresh AST per call.
 * `name` is the display label from the mock; `id` is the stable key.
 */
export const EXAMPLES: ReadonlyArray<{
  id: string;
  name: string;
  desc: string;
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
 * resets itself; loading never runs the query (a click fills the editor,
 * and the user decides when to run).
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
}

/** IPC boundary the query runner uses to call the core. */
export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
}

/**
 * Execution state pushed to the view on every change. Only the latest result
 * is retained. The controller tracks its own in-flight run; the view combines
 * it with the global busy state.
 */
export interface QueryState {
  running: boolean;
  error: string | null;
  result: QueryOutput | null;
}

/** The state before any run: nothing in flight, no results. */
export function emptyQueryState(): QueryState {
  return { running: false, error: null, result: null };
}

/**
 * Creates a query runner scoped to one editor. `run` invokes `run_query` with
 * the `queryArgv` arguments and keeps only the latest result; a second `run`
 * while one is in flight is ignored — the core serializes bazel work, and the
 * view decides when to call again. Errors thrown by `invoke` (busy/spawn
 * class) land in `error`; a non-zero exitCode is data, not an error. Starting
 * a run clears the previous result and error. `clear` drops the stored result
 * and error and invalidates the in-flight response — an edited query never
 * shows a reply fetched for older inputs — without releasing the running
 * slot. Responses arriving after `dispose()` are dropped entirely; a run
 * still in flight finishes on its own — there is no stop in v1.
 */
export function createQuery(
  t: Transport,
  changed: (s: QueryState) => void,
): {
  run(expr: string, view: "raw" | "graph", flags: CommandFlags): Promise<void>;
  clear(): void;
  dispose(): void;
} {
  let state = emptyQueryState();
  let disposed = false;
  let acceptResult = false;
  function update(patch: Partial<QueryState>) {
    if (disposed) return;
    state = { ...state, ...patch };
    changed(state);
  }
  async function run(expr: string, view: "raw" | "graph", flags: CommandFlags): Promise<void> {
    if (disposed || state.running) return;
    acceptResult = true;
    update({ running: true, error: null, result: null });
    try {
      const result = await t.invoke<QueryOutput>(COMMANDS.runQuery, queryArgv(expr, view, flags));
      update({ running: false, result: acceptResult ? result : null });
    } catch (error) {
      update({ running: false, error: acceptResult ? String(error) : null });
    }
  }
  function clear() {
    acceptResult = false;
    update({ result: null, error: null });
  }
  function dispose() {
    disposed = true;
  }
  return { run, clear, dispose };
}
