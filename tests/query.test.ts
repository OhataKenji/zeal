import { test } from "node:test";
import assert from "node:assert/strict";
import {
  applyExample,
  codeDiverged,
  commandLine,
  createQuery,
  DEFAULT_FLAGS,
  defaultNodeFor,
  emptyQueryState,
  EXAMPLES,
  FUNCTIONS,
  isExprHole,
  keyFor,
  KIND_GROUPS,
  kindOf,
  makeCall,
  parse,
  queryArgv,
  serialize,
  switchKind,
  target,
  validate,
  type CommandFlags,
  type Expr,
  type QueryOutput,
  type QueryState,
} from "../src/lib/query.ts";

function parseOk(text: string): Expr {
  const r = parse(text);
  if (!r.ok) assert.fail(`expected to parse ${text}: ${r.error}`);
  return r.expr;
}
function parseErr(text: string) {
  const r = parse(text);
  if (r.ok) assert.fail(`expected a parse error: ${text}`);
  return r;
}
function text(root: Expr): string {
  const r = serialize(root);
  if (!r.ok) assert.fail(`expected to serialize: ${r.error}`);
  return r.text;
}
function callNode(e: Expr): Extract<Expr, { kind: "call" }> {
  if (e.kind !== "call") assert.fail("expected a call node");
  return e;
}
function binaryNode(e: Expr): Extract<Expr, { kind: "binary" }> {
  if (e.kind !== "binary") assert.fail("expected a binary node");
  return e;
}
function targetNode(v: Expr | string | null): Extract<Expr, { kind: "target" }> {
  if (typeof v !== "object" || v === null || v.kind !== "target")
    assert.fail("expected a target node");
  return v;
}

test("the catalog holds the 16 supported functions with their schemas", () => {
  assert.deepEqual(FUNCTIONS, {
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
  });
  assert.equal(Object.keys(FUNCTIONS).length, 16);
});

test("kind groups list target, every function, then the set operations", () => {
  const fnNames = [
    "deps",
    "rdeps",
    "somepath",
    "allpaths",
    "kind",
    "filter",
    "attr",
    "labels",
    "tests",
    "buildfiles",
    "loadfiles",
    "executables",
    "siblings",
    "same_pkg_direct_rdeps",
    "some",
    "visible",
  ];
  assert.deepEqual(KIND_GROUPS, [
    ["Expressions", [["target", "Target pattern"]]],
    ["Functions", fnNames.map((f) => [f, f])],
    [
      "Set operations",
      [
        ["intersect", "intersect"],
        ["union", "union"],
        ["except", "except"],
      ],
    ],
  ]);
});

test("constructors fill schema defaults and defaultNodeFor covers every kind", () => {
  assert.equal(text(target()), '""');
  assert.equal(text(target("//a:b")), "//a:b");
  const rdeps = callNode(makeCall("rdeps"));
  assert.equal(targetNode(rdeps.args[0]).text, "//...");
  assert.equal(targetNode(rdeps.args[1]).text, "");
  assert.equal(rdeps.args[2], null);
  assert.equal(text(rdeps), 'rdeps(//..., "")');
  assert.equal(text(makeCall("deps")), 'deps("")');
  assert.equal(text(defaultNodeFor("target")), '""');
  assert.equal(text(defaultNodeFor("deps")), 'deps("")');
  assert.equal(text(defaultNodeFor("union")), '("" union "")');
  // keep fills the wrapArg slot, wherever the schema puts it.
  const kept = target("//keep:me");
  assert.equal(text(makeCall("deps", kept)), "deps(//keep:me)");
  assert.equal(text(makeCall("rdeps", kept)), "rdeps(//..., //keep:me)");
  assert.equal(text(makeCall("attr", kept)), 'attr(size, "^small$", //keep:me)');
});

const EXPECTED: Record<string, string> = {
  commonDeps: "(deps(//app:server) intersect deps(//tools:cli))",
  rdepsKind: 'kind(".*_test rule", rdeps(//..., //app/lib:parser))',
  deps: "deps(//app:server)",
  somepath: "somepath(//app:server, //app/lib:parser)",
  onlyServer: "(deps(//app:server) except deps(//tools:cli))",
  smallTestSources: 'labels(srcs, attr(size, "^small$", //app/...))',
  suiteTests: "tests(//app:all_tests)",
  rdepsTests: "tests(rdeps(//..., //app/lib:parser))",
  allpaths: "allpaths(//app:server, //app/lib:parser)",
  buildfiles: "buildfiles(deps(//app:server))",
};

test("every example serializes to its golden text and parses back", () => {
  assert.deepEqual(
    EXAMPLES.map((e) => e.id),
    Object.keys(EXPECTED),
  );
  for (const { id, name, build } of EXAMPLES) {
    assert.ok(name.length > 0, `label for ${id}`);
    assert.deepEqual(serialize(build()), { ok: true, text: EXPECTED[id] }, `serialize ${id}`);
    const parsed = parse(EXPECTED[id]);
    assert.ok(parsed.ok, `parse ${id}`);
    assert.deepEqual(
      serialize(parsed.expr),
      { ok: true, text: EXPECTED[id] },
      `re-serialize ${id}`,
    );
  }
});

test("applyExample builds a fresh tree for a known name only", () => {
  const built = applyExample("deps");
  assert.ok(built);
  assert.equal(text(built), "deps(//app:server)");
  assert.equal(applyExample("no-such-example"), null);
  assert.deepEqual(DEFAULT_FLAGS, { noImplicit: true, noTool: false });
});

test("safe words serialize bare; keywords, spaces and leading -/* are quoted", () => {
  assert.equal(text(target("//app:server")), "//app:server");
  assert.equal(text(target("a*b")), "a*b");
  for (const kw of ["except", "in", "intersect", "let", "set", "union"]) {
    assert.equal(text(target(kw)), `"${kw}"`, kw);
  }
  assert.equal(text(target("has space")), '"has space"');
  assert.equal(text(target("-leading")), '"-leading"');
  assert.equal(text(target("*leading")), '"*leading"');
  assert.equal(text(target("")), '""');
});

test("quoting prefers double quotes, falls back to single, rejects both", () => {
  assert.equal(text(target("it's")), '"it\'s"');
  assert.equal(text(target('has"dq')), "'has\"dq'");
  const both = serialize(target(`a'b"c`));
  assert.equal(both.ok, false);
  if (!both.ok) assert.match(both.error, /both quote types/);
});

test("pattern arguments always serialize quoted, even when safe", () => {
  const f = callNode(makeCall("filter"));
  f.args[0] = "abc";
  assert.equal(text(f), 'filter("abc", "")');
  const k = callNode(makeCall("kind"));
  assert.equal(text(k), 'kind(".*_test rule", "")');
  // Other word arguments are not force-quoted.
  const a = callNode(makeCall("attr"));
  a.args[0] = "visibility";
  assert.equal(text(a), 'attr(visibility, "^small$", "")');
});

test("binary nodes always serialize parenthesized and ops normalize to words", () => {
  assert.equal(text(parseOk("//a ^ //b")), "(//a intersect //b)");
  assert.equal(text(parseOk("//a + //b")), "(//a union //b)");
  assert.equal(text(parseOk("//a - //b")), "(//a except //b)");
  // Same precedence, left associative across mixed operators.
  assert.equal(text(parseOk("//a union //b except //c")), "((//a union //b) except //c)");
  assert.equal(
    text(parseOk("//a intersect //b intersect //c")),
    "((//a intersect //b) intersect //c)",
  );
  // Redundant parens are dropped on the round trip.
  assert.equal(text(parseOk("((//a))")), "//a");
  assert.equal(text(parseOk("(//a union //b)")), "(//a union //b)");
  assert.equal(text(parseOk("deps(//a union //b)")), "deps((//a union //b))");
});

test("parser reads quoted words, optional ints and the @@/+/ word case", () => {
  assert.equal(text(parseOk("'//a'")), "//a");
  assert.equal(text(parseOk('"a b"')), '"a b"');
  const deps = callNode(parseOk("deps(//a, 2)"));
  assert.equal(deps.args[1], "2");
  assert.equal(text(deps), "deps(//a, 2)");
  const bare = callNode(parseOk("deps(//a)"));
  assert.equal(bare.args[1], null);
  // `+` continues a word that starts with @@; elsewhere it is the union op.
  assert.equal(text(parseOk("@@main+canonical//pkg:t")), '"@@main+canonical//pkg:t"');
  assert.equal(text(parseOk("//a+//b")), "(//a union //b)");
});

test("parser rejects set, let and $name variables", () => {
  const set = parseErr("set(//a, //b)");
  assert.equal(set.pos, 0);
  assert.match(set.error, /expected word/);
  const letIn = parseErr("let x");
  assert.equal(letIn.pos, 0);
  assert.match(letIn.error, /expected word/);
  // The full `let x = //a in //b` form is rejected even earlier: `=` is not a
  // valid word character and fails in the tokenizer.
  const letFull = parseErr("let x = //a in //b");
  assert.equal(letFull.pos, 6);
  const variable = parseErr("$x");
  assert.equal(variable.pos, 0);
  assert.match(variable.error, /variables are not supported in Builder/);
  const inArg = parseErr("deps($x)");
  assert.equal(inArg.pos, 5);
  assert.match(inArg.error, /variables are not supported in Builder/);
  // Quoted `$x` is a literal word, not a variable reference, and a `$x`
  // word never serializes bare back into a variable reference.
  assert.ok(parse('"$x"').ok);
  assert.equal(text(target("$x")), '"$x"');
  const letArg = parseErr("deps(let)");
  assert.equal(letArg.pos, 5);
});

test("parser reports positions for other malformed input", () => {
  const unknown = parseErr("fizzle(//a)");
  assert.equal(unknown.pos, 0);
  assert.match(unknown.error, /fizzle is not supported by this Builder/);
  const unterminated = parseErr("'abc");
  assert.equal(unterminated.pos, 4);
  assert.match(unterminated.error, /Unterminated quoted word/);
  const trailingOp = parseErr("//a union");
  assert.equal(trailingOp.pos, 9);
  const closed = parseErr("//a)");
  assert.equal(closed.pos, 3);
  assert.match(closed.error, /expected eof/);
  const leadingDash = parseErr("-//a");
  assert.equal(leadingDash.pos, 0);
  const empty = parseErr("");
  assert.equal(empty.pos, 0);
  const missingComma = parseErr("rdeps(//a)");
  assert.equal(missingComma.pos, 9);
  assert.match(missingComma.error, /expected ,/);
});

test("validate flags required exprs, leading -/* and unquotable words", () => {
  const deps = callNode(makeCall("deps"));
  const inner = targetNode(deps.args[0]);
  const errs = validate(deps);
  assert.equal(errs.get(`${inner.id}:text`), "Required");
  assert.equal(errs.size, 1);

  const dash = target("-x");
  assert.equal(
    validate(dash).get(`${dash.id}:text`),
    "Targets cannot start with - or *. Use //pkg:target syntax.",
  );
  const star = target("*x");
  assert.equal(
    validate(star).get(`${star.id}:text`),
    "Targets cannot start with - or *. Use //pkg:target syntax.",
  );
  const ctrl = target("a\x07b");
  assert.equal(validate(ctrl).get(`${ctrl.id}:text`), "Control characters are not allowed");
  const quotes = target(`a'b"c`);
  assert.equal(validate(quotes).get(`${quotes.id}:text`), "The word contains both quote types");
  assert.equal(validate(parseOk("deps(//a)")).size, 0);
});

test("validate flags scalar call arguments by node id and index", () => {
  const attr = callNode(makeCall("attr"));
  targetNode(attr.args[2]).text = "//a";
  attr.args[0] = "bad\nname";
  assert.equal(validate(attr).get(`${attr.id}:0`), "Control characters are not allowed");
  attr.args[0] = `a'b"c`;
  assert.equal(validate(attr).get(`${attr.id}:0`), "The word contains both quote types");
});

test("validate enforces int syntax, range and minimum", () => {
  const some = callNode(makeCall("some"));
  targetNode(some.args[0]).text = "//a";
  assert.equal(validate(some).size, 0, "unset optional int is fine");
  some.args[1] = "";
  assert.equal(validate(some).size, 0, "empty optional int is fine");
  some.args[1] = "0";
  assert.equal(validate(some).get(`${some.id}:1`), "Must be at least 1");
  some.args[1] = "1";
  assert.equal(validate(some).size, 0);
  some.args[1] = "abc";
  assert.equal(validate(some).get(`${some.id}:1`), "Enter a non-negative integer");
  some.args[1] = "-2";
  assert.equal(validate(some).get(`${some.id}:1`), "Enter a non-negative integer");
  some.args[1] = "2147483648";
  assert.equal(validate(some).get(`${some.id}:1`), "Must be at most 2147483647");
  some.args[1] = "2147483647";
  assert.equal(validate(some).size, 0);
  const deps = callNode(makeCall("deps"));
  targetNode(deps.args[0]).text = "//a";
  deps.args[1] = "0";
  assert.equal(validate(deps).size, 0, "depth minimum is 0");
});

test("switchKind carries the wrapArg expression between functions", () => {
  const deps = parseOk("deps(//a:b)");
  const rdeps = callNode(switchKind(deps, "rdeps"));
  assert.equal(rdeps.fn, "rdeps");
  assert.equal(rdeps.id, deps.id);
  assert.equal(text(rdeps), "rdeps(//..., //a:b)");

  const somepath = callNode(switchKind(deps, "somepath"));
  assert.equal(text(somepath), 'somepath(//a:b, "")');

  const allpaths = callNode(switchKind(parseOk("somepath(//a, //b)"), "allpaths"));
  assert.equal(text(allpaths), "allpaths(//a, //b)");

  // Non-wrap arguments carry over only on a name+type match: depth survives
  // rdeps→deps, the universe does not.
  const back = callNode(switchKind(parseOk("rdeps(//u, //e, 3)"), "deps"));
  assert.equal(text(back), "deps(//e, 3)");

  const filter = callNode(switchKind(parseOk('kind(".*_test rule", //x)'), "filter"));
  assert.equal(text(filter), 'filter(".*_test rule", //x)');
});

test("switchKind keeps binary children, replaces across kinds", () => {
  const b = binaryNode(parseOk("//a intersect //b"));
  const u = binaryNode(switchKind(b, "union"));
  assert.equal(u.id, b.id);
  assert.equal(u.left, b.left);
  assert.equal(u.right, b.right);
  assert.equal(text(u), "(//a union //b)");

  const deps = parseOk("deps(//a:b)");
  const asBinary = binaryNode(switchKind(deps, "except"));
  assert.equal(text(asBinary), '("" except "")');
  assert.notEqual(asBinary.id, deps.id);

  const asCall = callNode(switchKind(b, "deps"));
  assert.equal(text(asCall), 'deps("")');
  assert.equal(text(switchKind(deps, "target")), '""');
  assert.equal(text(switchKind(target("//a"), "deps")), 'deps("")');
  assert.equal(switchKind(deps, "deps"), deps, "same kind is a no-op");
  assert.equal(kindOf(deps), "deps");
  assert.equal(kindOf(b), "intersect");
  assert.equal(kindOf(target("//a")), "target");
});

test("commandLine quotes the expr, mirrors the executed argv and appends flags", () => {
  const q = "deps(//a)";
  const on: CommandFlags = { noImplicit: true, noTool: false };
  assert.equal(
    commandLine(q, "raw", on),
    "bazel query 'deps(//a)' --output=label --noimplicit_deps",
  );
  assert.equal(
    commandLine(q, "raw", { noImplicit: true, noTool: true }),
    "bazel query 'deps(//a)' --output=label --noimplicit_deps --notool_deps",
  );
  assert.equal(
    commandLine(q, "graph", { noImplicit: false, noTool: false }),
    "bazel query 'deps(//a)' --output=graph --nograph:factored",
  );
  assert.equal(
    commandLine(q, "graph", { noImplicit: false, noTool: true }),
    "bazel query 'deps(//a)' --output=graph --nograph:factored --notool_deps",
  );
  assert.equal(
    commandLine("a'b", "raw", { noImplicit: false, noTool: false }),
    "bazel query 'a'\\''b' --output=label",
  );
});

test("queryArgv and keyFor carry flags and the chosen view", () => {
  // The Raw view asks the core for --output=label; Graph for --output=graph.
  assert.deepEqual(queryArgv("deps(//a)", "raw", { noImplicit: true, noTool: false }), {
    expr: "deps(//a)",
    output: "label",
    noImplicitDeps: true,
    noToolDeps: false,
  });
  assert.deepEqual(queryArgv("deps(//a)", "graph", { noImplicit: true, noTool: true }), {
    expr: "deps(//a)",
    output: "graph",
    noImplicitDeps: true,
    noToolDeps: true,
  });
  assert.equal(keyFor("deps(//a)", "raw", { noImplicit: true, noTool: false }), "10|raw|deps(//a)");
  assert.equal(keyFor("deps(//a)", "raw", { noImplicit: true, noTool: true }), "11|raw|deps(//a)");
});

test("isExprHole and codeDiverged mirror the mock's editor state checks", () => {
  const hole = target("");
  assert.equal(isExprHole(hole, new Set()), true);
  assert.equal(isExprHole(hole, new Set([hole.id])), false);
  assert.equal(isExprHole(target("//a"), new Set()), false);
  assert.equal(isExprHole(parseOk("deps(//a)"), new Set()), false);

  const deps = parseOk("deps(//a)");
  assert.equal(codeDiverged(deps, "deps(//a)"), false);
  assert.equal(codeDiverged(deps, "  deps(//a)\n"), false);
  assert.equal(codeDiverged(deps, "deps(//b)"), true);
  // An unserializable tree compares as the empty string.
  const broken = target(`a'b"c`);
  assert.equal(codeDiverged(broken, ""), false);
  assert.equal(codeDiverged(broken, "deps(//a)"), true);
});

const FLAGS: CommandFlags = { noImplicit: true, noTool: false };
const OUTPUT: QueryOutput = {
  exitCode: 0,
  stdout: "//a:b\n",
  truncated: false,
  stderrTail: "",
  timedOut: false,
  stopped: false,
};

function harness() {
  let state = emptyQueryState();
  const calls: Array<{
    command: string;
    args?: Record<string, unknown>;
    resolve: (value: unknown) => void;
    reject: (error: unknown) => void;
  }> = [];
  const runner = createQuery(
    {
      invoke: <T>(command: string, args?: Record<string, unknown>) =>
        new Promise<T>((resolve, reject) =>
          calls.push({
            command,
            args,
            resolve: (v: unknown) => resolve(v as T),
            reject,
          }),
        ),
    },
    (next: QueryState) => (state = next),
  );
  return {
    runner,
    calls,
    get state() {
      return state;
    },
  };
}

test("run invokes run_query and stores the reply under the chosen view", async () => {
  const h = harness();
  const pending = h.runner.run("deps(//a)", "raw", FLAGS);
  assert.equal(h.state.running, true);
  assert.equal(h.state.stopping, false);
  assert.equal(h.state.error, null);
  assert.equal(h.calls.length, 1);
  assert.equal(h.calls[0].command, "run_query");
  assert.deepEqual(h.calls[0].args, queryArgv("deps(//a)", "raw", FLAGS));
  h.calls[0].resolve(OUTPUT);
  await pending;
  assert.equal(h.state.running, false);
  assert.equal(h.state.error, null);
  assert.deepEqual(h.state.results.raw, {
    ...OUTPUT,
    key: keyFor("deps(//a)", "raw", FLAGS),
  });
  assert.equal(h.state.results.graph, undefined);
});

test("a non-zero exitCode is stored as a result, not an error", async () => {
  const h = harness();
  const pending = h.runner.run("deps(//missing)", "raw", FLAGS);
  h.calls[0].resolve({
    ...OUTPUT,
    exitCode: 7,
    stdout: "",
    stderrTail: "ERROR: no such target",
  });
  await pending;
  assert.equal(h.state.running, false);
  assert.equal(h.state.error, null);
  assert.equal(h.state.results.raw?.exitCode, 7);
  assert.equal(h.state.results.raw?.stderrTail, "ERROR: no such target");
});

test("invoke failures land in error and leave results empty", async () => {
  const h = harness();
  const pending = h.runner.run("deps(//a)", "raw", FLAGS);
  h.calls[0].reject(new Error("bazel is busy"));
  await pending;
  assert.equal(h.state.running, false);
  assert.match(h.state.error ?? "", /busy/);
  assert.deepEqual(h.state.results, {});
  // Retrying clears the error once the reply lands.
  const retry = h.runner.run("deps(//a)", "raw", FLAGS);
  assert.equal(h.state.error, null);
  h.calls[1].resolve(OUTPUT);
  await retry;
  assert.equal(h.state.results.raw?.exitCode, 0);
});

test("a second run while one is in flight is ignored", async () => {
  const h = harness();
  const first = h.runner.run("deps(//a)", "raw", FLAGS);
  const ignored = h.runner.run("deps(//b)", "graph", FLAGS);
  await ignored;
  assert.equal(h.calls.length, 1);
  h.calls[0].resolve(OUTPUT);
  await first;
  assert.equal(h.state.results.raw?.key, keyFor("deps(//a)", "raw", FLAGS));
  assert.equal(h.state.results.graph, undefined);
});

test("each view stores its own result under its own key", async () => {
  const h = harness();
  const rawRun = h.runner.run("deps(//a)", "raw", FLAGS);
  h.calls[0].resolve(OUTPUT);
  await rawRun;
  const graphRun = h.runner.run("deps(//a)", "graph", FLAGS);
  h.calls[1].resolve({ ...OUTPUT, stdout: "digraph {}" });
  await graphRun;
  assert.equal(h.state.results.raw?.stdout, "//a:b\n");
  assert.equal(h.state.results.graph?.stdout, "digraph {}");
  // Staleness: a stored key matches only the same expr, view and flags.
  assert.equal(h.state.results.raw?.key, keyFor("deps(//a)", "raw", FLAGS));
  assert.notEqual(
    h.state.results.raw?.key,
    keyFor("deps(//a)", "raw", { noImplicit: false, noTool: false }),
  );
  assert.notEqual(h.state.results.raw?.key, keyFor("deps(//a)", "graph", FLAGS));
  assert.notEqual(h.state.results.raw?.key, keyFor("deps(//b)", "raw", FLAGS));
});

test("stop invokes stop_query and holds stopping until the reply lands", async () => {
  const h = harness();
  const pending = h.runner.run("deps(//a)", "raw", FLAGS);
  const stopping = h.runner.stop();
  assert.equal(h.state.stopping, true);
  assert.equal(h.calls.length, 2);
  assert.equal(h.calls[1].command, "stop_query");
  h.calls[1].resolve(undefined);
  await stopping;
  assert.equal(h.state.stopping, true);
  h.calls[0].resolve({ ...OUTPUT, stopped: true });
  await pending;
  assert.equal(h.state.running, false);
  assert.equal(h.state.stopping, false);
  assert.equal(h.state.results.raw?.stopped, true);
});

test("stop is a no-op when nothing is in flight", async () => {
  const h = harness();
  await h.runner.stop();
  assert.equal(h.calls.length, 0);
});

test("dispose drops a late reply, stops the query, and ignores further calls", async () => {
  const h = harness();
  const pending = h.runner.run("deps(//a)", "raw", FLAGS);
  h.runner.dispose();
  // The discarded run's result is dropped anyway; stop_query frees the slot.
  assert.equal(h.calls.length, 2);
  assert.equal(h.calls[1].command, "stop_query");
  h.calls[1].resolve(undefined);
  h.calls[0].resolve(OUTPUT);
  await pending;
  assert.deepEqual(h.state.results, {});
  const after = h.runner.run("deps(//a)", "raw", FLAGS);
  await after;
  assert.equal(h.calls.length, 2);
});
