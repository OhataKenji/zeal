// Browser-only development preview. Never loaded by a production build.
import { isTauri } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import historyJson from "./preview/history.json";
import queriesJson from "./preview/queries.json";
import workspaceJson from "./preview/workspace.json";
import type {
  ActionConfiguration,
  AppState,
  Bep,
  ConfiguredDep,
  Edge,
  FileEntry,
  InvocationRecord,
  ModulesSnapshot,
  OwnedInvocation,
  Profile,
  Target,
  TargetAction,
  TargetActions,
  TargetDetail,
  TargetIndex,
  WorkspaceInfo,
  WorkspaceState,
} from "./lib/types";
import type { OutputFormat, QueryOutput, QueryRequest } from "./lib/query.ts";
import type { AnalyticsFilter } from "./lib/analytics";
import { previewAnalytics, sampleAnalyticsActivity } from "./preview/analytics.ts";
import { interpretConsoleInput } from "./preview/console.ts";
import type { CommandName } from "./lib/commands.ts";

/** One captured invocation: the history row plus its saved log, BEP and
 * profile. The profile's lanes and phases are the selected trace events;
 * they keep their original timestamps. */
interface CapturedInvocation {
  record: {
    id: string;
    cmd: string;
    pattern: string;
    startedAt: number;
    finishedAt: number;
    exitCode: number;
  };
  log: string;
  bep: Bep;
  profile: Profile;
}

/** The captured workspace: real `bazel query`/`mod graph`/`aquery`/`cquery`
 * output for the parcel-desk example, exported for the preview. Long action
 * argument lists are abbreviated for the preview. */
interface WorkspaceCapture {
  info: WorkspaceInfo;
  targets: Target[];
  edges: Edge[];
  files: FileEntry[];
  modules: ModulesSnapshot;
  actions: TargetAction[];
  outputs: Record<string, string[]>;
  transitions: Record<string, ConfiguredDep[]>;
}

const WORKSPACE = "/demo/parcel-desk";
const history: CapturedInvocation[] = historyJson;
const capture: WorkspaceCapture = workspaceJson;
const NOW = Date.now();
// History timestamps are shifted so the newest capture ends "now"; profile
// phase/thread timings inside each entry stay at their original values.
const shift = NOW - Math.max(...history.map((h) => h.record.finishedAt));
type PreviewRecord = InvocationRecord & {
  workspace: string;
  cmd: string;
  pattern: string;
  startedAt: number;
};
const records: PreviewRecord[] = history
  .map(({ record: r }) => ({
    ...r,
    workspace: WORKSPACE,
    startedAt: r.startedAt + shift,
    finishedAt: r.finishedAt + shift,
  }))
  .toSorted((a, b) => b.startedAt - a.startedAt);
const captured = new Map(history.map((h) => [h.record.id, h]));
// Deterministic synthetic rows for the Analytics chart only; History and
// Timing keep serving just the captured invocations.
const analyticsSamples = sampleAnalyticsActivity(
  history.map(({ record }) => record),
  WORKSPACE,
  NOW,
);

const SNAPSHOT: TargetIndex = {
  meta: { fetchedAt: NOW, status: "ok" },
  targets: capture.targets,
};
const state: AppState = {
  revision: 0,
  besError: null,
  besEndpoint: "grpc://127.0.0.1:4718",
  bazel: capture.info.version,
  lastWorkspace: WORKSPACE,
  workspaces: [
    {
      workspace: WORKSPACE,
      scope: { include: ["//..."] },
    },
  ],
  invocations: [],
};

function emitState() {
  state.revision += 1;
  void emit("app-state", structuredClone(state));
}

function wsEntry(workspace: unknown): WorkspaceState {
  const id = String(workspace ?? "");
  const entry = state.workspaces.find((item) => item.workspace === id);
  if (!entry) throw new Error(`Preview workspace is not open: ${id}`);
  return entry;
}

function ownedFor(id: string): OwnedInvocation | undefined {
  return state.invocations.find((entry) => entry.id === id);
}
const consoleOutput = new Map<string, string>();
const consoleInput = new Map<string, string>();

function readConsole(workspace: string, id: string, offset: number) {
  wsEntry(workspace);
  const owned = ownedFor(id);
  if (!owned || !owned.console) throw new Error("Preview invocation not found");
  if (owned.workspace !== workspace) {
    throw new Error("Preview invocation belongs to another workspace");
  }
  const bytes = new TextEncoder().encode(consoleOutput.get(id) ?? "");
  const data = Array.from(bytes.slice(offset, offset + 64 * 1024));
  return {
    data,
    offset: offset + data.length,
  };
}

function writeConsole(workspace: string, id: string, data: unknown) {
  wsEntry(workspace);
  const owned = ownedFor(id);
  if (owned && owned.workspace !== workspace) {
    throw new Error("Preview invocation belongs to another workspace");
  }
  if (!owned) throw new Error("Preview process has ended");
  if (!owned.console || !owned.stoppable || owned.stopping) {
    throw new Error("Preview console is not writable");
  }
  if (!Array.isArray(data)) throw new Error("Preview console input must be a byte array.");
  const text = new TextDecoder().decode(Uint8Array.from(data));
  const result = interpretConsoleInput(consoleInput.get(id) ?? "", text);
  consoleInput.set(id, result.input);
  consoleOutput.set(id, (consoleOutput.get(id) ?? "") + result.output);
  if (result.exitCode !== null) finishOwned(id, result.exitCode);
  return data.length;
}

function getInvocation(workspace: string, id: unknown) {
  const record = records.find((inv) => inv.id === id && inv.workspace === workspace);
  if (!record) throw new Error("Preview invocation not found");
  const item = captured.get(record.id);
  return {
    record,
    log: item ? { status: "ok" as const, data: item.log } : { status: "missing" as const },
    bep: item ? { status: "ok" as const, data: item.bep } : { status: "missing" as const },
  };
}

/** Records the simulated run's end on its history row, if it is still open. */
function finishOwned(id: string, exitCode = 0) {
  const record = records.find((inv) => inv.id === id);
  const finished = record !== undefined && record.finishedAt === undefined;
  if (record && finished) {
    record.finishedAt = Date.now();
    record.exitCode = exitCode;
  }
  state.invocations = state.invocations.filter((entry) => entry.id !== id);
  consoleOutput.delete(id);
  consoleInput.delete(id);
  emitState();
  // Query runs are not history rows; only a finished record needs a list refresh.
  if (finished) void emit("invocations-changed");
}

function startInvocation(workspace: string, cmd: string, pattern: string) {
  wsEntry(workspace);
  const id = `preview-${crypto.randomUUID()}`;
  const now = Date.now();
  // Simulated runs have no captured BEP or profile: the record starts without
  // finish timing and both detail reads remain missing after it ends.
  const record: PreviewRecord = {
    id,
    workspace,
    cmd,
    pattern,
    startedAt: now,
  };
  records.unshift(record);
  const owned: OwnedInvocation = {
    id,
    workspace,
    cmd,
    pattern,
    startedAt: record.startedAt,
    stoppable: true,
    stopping: false,
    console: true,
  };
  state.invocations.unshift(owned);
  consoleOutput.set(
    id,
    `\x1b[32mINFO:\x1b[0m bazel ${record.cmd} ${record.pattern}\r\nPreview Console — simulated run, not a real Bazel process\r\n`,
  );
  if (cmd !== "run") {
    // build/test finish on their own; `run` stays live until ^C/^D or Stop.
    setTimeout(() => finishOwned(id), 800);
  }
  emitState();
  void emit("invocations-changed");
  return id;
}

function stopInvocation(workspace: string, id: string) {
  wsEntry(workspace);
  const owned = ownedFor(id);
  if (owned && owned.workspace !== workspace) {
    throw new Error("Preview invocation belongs to another workspace");
  }
  if (!owned?.stoppable) return;
  owned.stopping = true;
  emitState();
  finishOwned(id, 130);
}

/** Default target configuration for rules without captured actions. */
const DEFAULT_CONFIGURATION = capture.actions.find((a) => a.configuration)?.configuration;

/** cquery prints each target's configuration as a short checksum; the
 * per-owner action configurations reproduce the captured assignment. */
const configByOwner = new Map<string, ActionConfiguration>();
for (const action of capture.actions) {
  if (action.configuration && !configByOwner.has(action.owner)) {
    configByOwner.set(action.owner, action.configuration);
  }
}
/** The configuration a target's actions ran under, or the capture-wide
 * default when the target has no captured actions. */
function ownerConfig(label: string): ActionConfiguration | undefined {
  return configByOwner.get(label) ?? DEFAULT_CONFIGURATION ?? undefined;
}
function targetConfig(label: string): string {
  return ownerConfig(label)?.checksum.slice(0, 7) ?? "";
}

/** The preview answers only patterns it can resolve against the captured
 * rule-target set exactly; anything else is refused rather than guessed.
 * `:*` variants are unsupported because they also match file targets, which
 * the capture does not include. */
function previewTargets(expr: string): string[] {
  const inner = /^set\((.*)\)$/.exec(expr)?.[1];
  if (inner !== undefined) return previewTargets(inner);
  const labels = new Set(capture.targets.map((t) => t.label));
  if (expr === "//...") {
    return capture.targets.map((t) => t.label);
  }
  if (labels.has(expr)) return [expr];
  const pkgAll = /^\/\/([^:]+):all$/.exec(expr);
  if (pkgAll) {
    return capture.targets.filter((t) => t.pkg === pkgAll[1]).map((t) => t.label);
  }
  const pkgTree = /^\/\/([^/]+(?:\/[^/]+)*)\/\.\.\.$/.exec(expr);
  if (pkgTree) {
    const prefix = pkgTree[1];
    return capture.targets
      .filter((t) => t.pkg === prefix || t.pkg.startsWith(`${prefix}/`))
      .map((t) => t.label);
  }
  const pkgName = /^\/\/([^:]+)$/.exec(expr);
  if (pkgName) {
    const name = pkgName[1].split("/").pop() ?? "";
    const label = `//${pkgName[1]}:${name}`;
    if (labels.has(label)) return [label];
  }
  throw new Error(
    `Preview cannot evaluate "${expr}"; it answers //..., //pkg/..., //pkg:all and exact labels from the captured workspace`,
  );
}

/** A sidebar preset captured verbatim from a real query run: the raw stdout
 * for one output mode, or the action outputs an aquery selected. */
interface QueryPreset {
  kind: string;
  expr: string;
  label?: string;
  graph?: string;
  transitions?: string;
  actionOutputs?: string[];
}
const QUERY_PRESETS = new Map(
  (queriesJson as QueryPreset[]).map((preset) => [`${preset.kind}\n${preset.expr}`, preset]),
);

function queryOk(stdout: string, actions: TargetActions | null = null): QueryOutput {
  return { exitCode: 0, stdout, stdoutTruncated: false, truncated: false, stderrTail: "", actions };
}

/** aquery replies carry the matched actions both parsed and as JSON text. */
function actionsOutput(matched: TargetAction[]): QueryOutput {
  const kept = matched.slice(0, 100);
  return queryOk(`${JSON.stringify({ actions: kept }, null, 2)}\n`, {
    actions: kept,
    truncated: matched.length > 100,
  });
}

/** Replays a preset's captured output for the requested mode; null when the
 * preset does not cover it, so the caller falls back to projection. */
function presetOutput(preset: QueryPreset, output: OutputFormat): QueryOutput | null {
  if (output === "label" && preset.label !== undefined) return queryOk(preset.label);
  if (output === "graph" && preset.graph !== undefined) return queryOk(preset.graph);
  if (output === "transitions" && preset.transitions !== undefined) {
    return queryOk(preset.transitions);
  }
  if (output === "jsonproto" && preset.actionOutputs !== undefined) {
    const wanted = new Set(preset.actionOutputs);
    return actionsOutput(
      capture.actions.filter(
        (action) => wanted.has(action.output) || action.outputs.some((path) => wanted.has(path)),
      ),
    );
  }
  return null;
}

/** Sample `run_query` reply over the captured graph: real labels, edges,
 * configuration checksums, transitions and actions — no query evaluation. */
function previewQueryOutput(request: QueryRequest): QueryOutput {
  const { kind, output, expr, flags, noImplicitDeps, noToolDeps } = request;
  if (kind !== "query") {
    if (String(flags ?? "").trim() !== "") {
      throw new Error(
        "Preview captured only the default Bazel configuration; cquery/aquery flags are not supported",
      );
    }
  } else if (!noImplicitDeps || noToolDeps) {
    throw new Error(
      "Preview target data was captured with --noimplicit_deps; dependency-scope flags are not supported",
    );
  }
  const preset = QUERY_PRESETS.get(`${kind}\n${expr}`);
  if (preset) {
    const replayed = presetOutput(preset, output);
    if (replayed) return replayed;
  }
  return projectedQueryOutput(request);
}

function unsupportedOutput(kind: string, output: OutputFormat): never {
  throw new Error(`Preview cannot answer ${kind} --output=${output}: unsupported output ${output}`);
}

function cqueryStdout(selected: string[], output: OutputFormat): string {
  if (output === "transitions") {
    return selected
      .flatMap((label) => [
        `${label} (${targetConfig(label)})`,
        ...(capture.transitions[label] ?? []).map(
          (dep) => `  ${dep.label}#${dep.transition ?? "NoTransition"}`,
        ),
      ])
      .join("\n")
      .concat("\n");
  }
  if (output === "label") {
    return selected
      .map((label) => `${label} (${targetConfig(label)})`)
      .join("\n")
      .concat("\n");
  }
  return unsupportedOutput("cquery", output);
}

/** Graph output lists every selected target as a node and only edges whose
 * endpoints are both selected, so a narrow pattern cannot acquire unrelated
 * targets through their edges. */
function graphStdout(selected: string[], selectedSet: Set<string>): string {
  const edges = capture.edges.filter((e) => selectedSet.has(e.from) && selectedSet.has(e.to));
  const body = [
    ...selected.map((label) => `  ${JSON.stringify(label)}`),
    ...edges.map((e) => `  ${JSON.stringify(e.from)} -> ${JSON.stringify(e.to)};`),
  ].join("\n");
  return `digraph mygraph {\n  node [shape=box];\n${body}\n}\n`;
}

function projectedQueryOutput({ kind, output, expr }: QueryRequest): QueryOutput {
  const selected = previewTargets(expr);
  const selectedSet = new Set(selected);
  if (kind === "aquery") {
    if (output !== "jsonproto") return unsupportedOutput(kind, output);
    const all = expr === "//..." || expr === "set(//...)";
    const matched = all ? capture.actions : capture.actions.filter((a) => selectedSet.has(a.owner));
    return actionsOutput(matched);
  }
  if (kind === "cquery") return queryOk(cqueryStdout(selected, output));
  if (output === "graph") return queryOk(graphStdout(selected, selectedSet));
  if (output === "label") return queryOk(selected.join("\n").concat("\n"));
  return unsupportedOutput(kind, output);
}

/** Installs mock core IPC for the development-only browser preview. */
export function installPreview() {
  if (!import.meta.env.DEV || isTauri()) throw new Error("Preview requires a development browser");
  if (new URLSearchParams(location.search).get("preview") === "empty") {
    state.lastWorkspace = null;
    state.workspaces = [];
  }
  function candidates(): string[] {
    const counts = new Map<string, { count: number; last: number }>();
    for (const inv of records
      .toSorted((a, b) => b.startedAt - a.startedAt || a.id.localeCompare(b.id))
      .slice(0, 100)) {
      const rank = counts.get(inv.workspace);
      if (rank) rank.count++;
      else counts.set(inv.workspace, { count: 1, last: inv.startedAt });
    }
    return [...counts]
      .sort(([a, x], [b, y]) => y.count - x.count || y.last - x.last || a.localeCompare(b))
      .slice(0, 5)
      .map(([path]) => path);
  }
  // Every command the app invokes needs a handler; the Record type makes a
  // missing one a typecheck error instead of a runtime surprise.
  const handlers: Record<CommandName, (payload: Record<string, unknown> | undefined) => unknown> = {
    get_state: () => structuredClone(state),
    get_modules: (payload) => {
      wsEntry(payload?.workspace);
      return structuredClone(capture.modules);
    },
    explain_module: (payload) => {
      wsEntry(payload?.workspace);
      const key = payload?.moduleKey;
      if (
        typeof key !== "string" ||
        !key ||
        key.length > 256 ||
        key.startsWith("-") ||
        /[^a-zA-Z0-9._+@~-]/.test(key)
      ) {
        throw new Error("Invalid module key");
      }
      const node = capture.modules.modules.find((item) => item.key === key);
      if (!node) throw new Error(`Unknown module: ${key}`);
      // `bazel mod explain` output was not captured; the module key followed
      // by its immediate parents from the module graph is the honest
      // explanation available.
      const parents = node.requiredBy.length > 0 ? node.requiredBy : ["(no parents)"];
      return {
        text: [key, ...parents].join("\n"),
        warning: capture.modules.warning,
      };
    },
    get_workspace_info: (payload): WorkspaceInfo => {
      const entry = wsEntry(payload?.workspace);
      return {
        version: capture.info.version,
        entries: capture.info.entries.map((row) =>
          row.key === "workspace" ? { ...row, value: entry.workspace } : { ...row },
        ),
      };
    },
    get_target_detail: (payload): TargetDetail => {
      wsEntry(payload?.workspace);
      const label = String(payload?.label);
      const selected = capture.targets.find((item) => item.label === label);
      if (!selected) throw new Error("Target not found");
      return structuredClone({
        target: selected,
        edges: capture.edges.filter((edge) => edge.from === label || edge.to === label),
        files: capture.files.filter((file) => file.owner === label),
        status: "ok",
      });
    },
    read_target: (payload) => {
      wsEntry(payload?.workspace);
      const label = String(payload?.label);
      if (!SNAPSHOT.targets.some((item) => item.label === label)) {
        throw new Error("Target not found");
      }
      const flags = String(payload?.flags ?? "");
      if (flags.trim() !== "") {
        throw new Error(
          "Preview captured only the default Bazel configuration; build flags are not supported",
        );
      }
      const step = payload?.step;
      if (step === "actions") {
        const matched = capture.actions.filter((a) => a.owner === label);
        return {
          actions: matched.slice(0, 100),
          truncated: matched.length > 100,
        };
      }
      if (step === "outputs") {
        const paths = capture.outputs[label] ?? [];
        return { paths: paths.slice(0, 10), truncated: paths.length > 10 };
      }
      if (step === "configuration") {
        const configuration = ownerConfig(label);
        return configuration
          ? { mnemonic: configuration.mnemonic, checksum: configuration.checksum }
          : null;
      }
      if (step === "transitions") {
        const deps = capture.transitions[label] ?? [];
        return { deps: deps.slice(0, 200), depsTruncated: deps.length > 200 };
      }
      throw new Error("Unknown target read step");
    },
    list_invocations: (payload) =>
      structuredClone(records.filter((inv) => inv.workspace === String(payload?.workspace ?? ""))),
    read_console: (payload) =>
      readConsole(
        String(payload?.workspace ?? ""),
        String(payload?.id),
        Number(payload?.offset ?? 0),
      ),
    resize_console: () => {},
    write_console: (payload) =>
      writeConsole(String(payload?.workspace ?? ""), String(payload?.id), payload?.data),
    get_invocation: (payload) => {
      const workspace = String(payload?.workspace ?? "");
      wsEntry(workspace);
      return getInvocation(workspace, payload?.id);
    },
    get_invocation_profile: (payload) => {
      const workspace = String(payload?.workspace ?? "");
      wsEntry(workspace);
      const record = records.find((inv) => inv.id === payload?.id && inv.workspace === workspace);
      if (!record) throw new Error("Preview invocation not found");
      const item = captured.get(record.id);
      return item ? { status: "ok" as const, data: item.profile } : { status: "missing" as const };
    },
    // The app sends this command with the AnalyticsFilter shape.
    get_analytics: (payload) => {
      const workspace = String(payload?.workspace ?? "");
      wsEntry(workspace);
      return previewAnalytics(
        [...analyticsSamples, ...records].filter((inv) => inv.workspace === workspace),
        (payload?.filter ?? {}) as Partial<AnalyticsFilter>,
        NOW,
      );
    },
    list_workspaces: () => structuredClone(candidates()),
    open_workspace: (payload) => {
      const workspace = String(payload?.path ?? WORKSPACE);
      const scope = structuredClone(payload?.scope) as WorkspaceState["scope"];
      let entry = state.workspaces.find((item) => item.workspace === workspace);
      if (!entry) {
        entry = { workspace, scope };
        state.workspaces.push(entry);
      } else {
        entry.scope = scope;
      }
      state.lastWorkspace = workspace;
      emitState();
      return { workspace, state: structuredClone(state) };
    },
    refresh: (payload) => {
      wsEntry(payload?.workspace);
      return { snapshot: structuredClone(SNAPSHOT), error: null };
    },
    start_invocation: (payload) =>
      startInvocation(
        String(payload?.workspace ?? ""),
        String(payload?.cmd),
        String(payload?.pattern),
      ),
    stop_invocation: (payload) =>
      stopInvocation(String(payload?.workspace ?? ""), String(payload?.id)),
    run_query: (payload) => {
      const request = payload?.request as QueryRequest;
      wsEntry(request.workspace);
      const { workspace, kind, expr } = request;
      const finished = previewQueryOutput(request);
      const owned: OwnedInvocation = {
        id: `preview-query-${crypto.randomUUID()}`,
        workspace,
        cmd: kind,
        pattern: expr,
        startedAt: Date.now(),
        stoppable: true,
        stopping: false,
        console: false,
      };
      state.invocations.unshift(owned);
      emitState();
      return new Promise((resolve, reject) => {
        setTimeout(() => {
          const stopped = owned.stopping;
          if (!stopped) finishOwned(owned.id);
          if (stopped) reject(new Error("Query stopped"));
          else resolve(finished);
        }, 800);
      });
    },
  };
  mockIPC(
    (command, args) => {
      if (command === "plugin:dialog|open") return WORKSPACE;
      const handler = handlers[command as CommandName];
      if (!handler) throw new Error(`Unsupported preview command: ${command}`);
      return handler(args as Record<string, unknown> | undefined);
    },
    { shouldMockEvents: true },
  );
}
