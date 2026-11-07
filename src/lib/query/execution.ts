import { COMMANDS } from "../commands.ts";

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
 * Execution state pushed to the view on every change. The selected result
 * is shown. The controller tracks its own in-flight run; the view combines
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
 * One editor retains the ten most recently used successful query results.
 * Selecting inputs restores a result without executing; run always refreshes it.
 * Editing invalidates an in-flight reply without releasing its slot.
 * The workspace-keyed editor owns this controller, so results never cross workspaces.
 */
export function createQuery(
  t: Transport,
  changed: (s: QueryState) => void,
): {
  select(expr: string, view: "raw" | "graph", flags: CommandFlags): void;
  run(expr: string, view: "raw" | "graph", flags: CommandFlags): Promise<void>;
  dispose(): void;
} {
  const cache = new Map<string, QueryOutput>();
  let state = emptyQueryState();
  let disposed = false;
  let acceptResult = false;
  function update(patch: Partial<QueryState>) {
    if (disposed) return;
    state = { ...state, ...patch };
    changed(state);
  }
  function select(expr: string, view: "raw" | "graph", flags: CommandFlags) {
    if (disposed) return;
    acceptResult = false;
    const key = JSON.stringify(queryArgv(expr, view, flags));
    const result = cache.get(key) ?? null;
    if (result) {
      cache.delete(key);
      cache.set(key, result);
    }
    update({ result, error: null });
  }
  async function run(expr: string, view: "raw" | "graph", flags: CommandFlags): Promise<void> {
    if (disposed || state.running) return;
    const args = queryArgv(expr, view, flags);
    const key = JSON.stringify(args);
    cache.delete(key);
    acceptResult = true;
    update({ running: true, error: null, result: null });
    let result: QueryOutput;
    try {
      result = await t.invoke<QueryOutput>(COMMANDS.runQuery, args);
    } catch (error) {
      update({ running: false, error: acceptResult ? String(error) : null });
      return;
    }
    update({ running: false });
    if (!acceptResult) return;
    update({ result });
    if (result.exitCode !== 0 || result.timedOut) return;
    cache.set(key, result);
    if (cache.size > 10) cache.delete(cache.keys().next().value!);
  }

  function dispose() {
    acceptResult = false;
    disposed = true;
  }
  return { select, run, dispose };
}
