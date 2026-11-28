// Browser-only development preview. Never loaded by a production build.
import { isTauri } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import historyRaw from "./preview/history.json?raw";
import queriesRaw from "./preview/queries.json?raw";
import workspaceRaw from "./preview/workspace.json?raw";
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
import { previewAnalytics } from "./preview/analytics.ts";
import { interpretConsoleInput } from "./preview/console.ts";
import type { CommandName } from "./lib/commands.ts";

/** One captured invocation: the history row plus its saved log and BEP. */
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
const history: CapturedInvocation[] = JSON.parse(historyRaw);
const capture: WorkspaceCapture = JSON.parse(workspaceRaw);
const queries: QueryPreset[] = JSON.parse(queriesRaw);
const NOW = Date.now();
// History timestamps are shifted so the newest capture ends "now"; trace
// event timings inside each entry stay at their original values.
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

type AnalyticsSample = Pick<
  PreviewRecord,
  "cmd" | "pattern" | "workspace" | "startedAt" | "finishedAt" | "exitCode"
> & {
  cacheHits: number;
  actionsTotal: number;
  analysisMs: number;
  executionMs: number;
};

function sampleDayCount(day: Date, ago: number): number {
  const weekend = day.getDay() === 0 || day.getDay() === 6;
  return weekend ? ago % 3 : [8, 12, 5, 17, 9, 14, 6, 11][ago % 8];
}

function sampleExitCode(sequence: number): number {
  return sequence % 37 === 0 ? 8 : sequence % 13 === 0 ? 1 : 0;
}

/** Synthetic prior days for the browser chart only; never invocation history. */
function sampleAnalyticsActivity(
  seeds: readonly CapturedInvocation["record"][],
  workspace: string,
  now: number,
): AnalyticsSample[] {
  const rows: AnalyticsSample[] = [];
  for (let ago = 29; ago >= 1 && seeds.length > 0; ago--) {
    const day = new Date(now);
    day.setDate(day.getDate() - ago);
    const count = sampleDayCount(day, ago);
    for (let i = 0; i < count; i++) {
      const seed = seeds[(ago + i) % seeds.length];
      const startedAt = day.setHours(8, 35 * i + (ago % 23), 0, 0);
      const elapsed = Math.max(
        100,
        Math.round(((seed.finishedAt - seed.startedAt) * (65 + ((ago * 11 + i * 17) % 90))) / 100),
      );
      const sequence = rows.length + 1;
      const exitCode = sampleExitCode(sequence);
      const actionsTotal = 40 + ((ago * 7 + i * 13) % 80);
      const cachePct = exitCode === 0 ? 55 + ((ago * 9 + i * 5) % 40) : 15 + ((ago * 7) % 35);
      const analysisMs = Math.max(50, Math.round(elapsed * (0.2 + ((ago + i) % 3) * 0.08)));
      rows.push({
        workspace,
        cmd: seed.cmd,
        pattern: seed.pattern,
        startedAt,
        finishedAt: startedAt + elapsed,
        exitCode,
        cacheHits: Math.round((actionsTotal * cachePct) / 100),
        actionsTotal,
        analysisMs,
        executionMs: Math.max(50, elapsed - analysisMs),
      });
    }
  }
  return rows;
}

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
  if (result.exitCode !== null) finishOwned(id);
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

function finishOwned(id: string) {
  state.invocations = state.invocations.filter((entry) => entry.id !== id);
  consoleOutput.delete(id);
  consoleInput.delete(id);
  emitState();
}

function startInvocation(workspace: string, cmd: string, pattern: string) {
  wsEntry(workspace);
  const id = `preview-${crypto.randomUUID()}`;
  const now = Date.now();
  // The BEP build result exists independently of the owned process: a `run`
  // program may outlive it, and its own output is never BEP history.
  const record: PreviewRecord = {
    id,
    workspace,
    cmd,
    pattern,
    startedAt: now,
    finishedAt: now,
    exitCode: 0,
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
    `\x1b[32mINFO:\x1b[0m bazel ${record.cmd} ${record.pattern}\r\nPreview Console — sample data\r\n${record.cmd === "run" ? "Number: " : "Build completed.\r\n"}`,
  );
  if (cmd !== "run") {
    // build/test finish on their own; `run` stays live until input or Stop.
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
  finishOwned(id);
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
const QUERY_PRESETS = new Map(queries.map((preset) => [`${preset.kind}\n${preset.expr}`, preset]));

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
 * preset does not cover it, so the caller falls back to captured data. */
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

/** Sample `run_query` reply: sidebar presets replay their captured output;
 * anything else falls back to the captured labels, edges, transitions and
 * actions — no query evaluation. */
function previewQueryOutput(request: QueryRequest): QueryOutput {
  const preset = QUERY_PRESETS.get(`${request.kind}\n${request.expr}`);
  if (preset) {
    const replayed = presetOutput(preset, request.output);
    if (replayed) return replayed;
  }
  return fallbackQueryOutput(request);
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

/** aquery answers with the actions owned by the exact label, or the whole
 * capture when the expression does not name one target. */
function aqueryOutput(expr: string, exact: boolean, output: OutputFormat): QueryOutput {
  if (output !== "jsonproto") return unsupportedOutput("aquery", output);
  const matched = exact ? capture.actions.filter((a) => a.owner === expr) : capture.actions;
  return actionsOutput(matched);
}

/** Baseline fallback for expressions without a captured preset: an exact
 * target label selects that owner, anything else selects the whole demo. */
function fallbackQueryOutput({ kind, output, expr }: QueryRequest): QueryOutput {
  const exact = capture.targets.some((t) => t.label === expr);
  const selected = exact ? [expr] : capture.targets.map((t) => t.label);
  const selectedSet = new Set(selected);
  if (kind === "aquery") return aqueryOutput(expr, exact, output);
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
      // `bazel mod explain` output was not captured; the root-to-module chain
      // built from the module graph stands in for it.
      return {
        text: [
          capture.modules.rootKey,
          ...node.requiredBy.filter((parent) => parent !== capture.modules.rootKey),
          key,
        ].join(" -> "),
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
    list_invocations: (payload) => ({
      saved: {
        Ok: structuredClone(
          records.filter((inv) => inv.workspace === String(payload?.workspace ?? "")),
        ),
      },
      diagnostics: [],
    }),
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
    // Same-origin stand-in for the native perfetto:// scheme; vite.config.ts
    // serves the fixture trace and mounts the verified assets in dev only.
    prepare_invocation_timing: (payload) => {
      const workspace = String(payload?.workspace ?? "");
      wsEntry(workspace);
      const record = records.find((inv) => inv.id === payload?.id && inv.workspace === workspace);
      const item = record && captured.get(record.id);
      // Mirrors the native missing-profile error so the viewer words it the same.
      if (!item) throw new Error("No saved timing profile is available for this invocation.");
      return `${location.origin}/__perfetto/embed.html#trace/${item.record.id}/profile.json.gz`;
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
