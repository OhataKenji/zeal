export interface ModuleNode {
  key: string;
  name: string;
  version: string;
  direct: boolean;
  requiredBy: string[];
  dependencies: string[];
}

export interface ModulesSnapshot {
  rootKey: string;
  rootLabel: string;
  modules: ModuleNode[];
  warning: string | null;
}

export interface ModuleExplanation {
  text: string;
  warning: string | null;
}

/** The one item currently selected in the target explorer. */
export type ExplorerSelection =
  | { kind: "target"; label: string }
  | { kind: "package"; path: string }
  | null;

/** Location attached to a build target when the core has source metadata. */
export interface TargetLocation {
  file: string;
  line: number;
}

/** One selectable Bazel target. Labels are the stable identity used by the UI. */
export interface Target {
  label: string;
  pkg: string;
  name: string;
  ruleClass: string;
  location: TargetLocation | null;
}

/** A dependency or data relationship between two target labels. */
export interface Edge {
  from: string;
  to: string;
  attr: string;
}

/** A workspace file and the target attribute that owns it. */
export interface FileEntry {
  path: string;
  owner: string;
  role: string;
}

/** Fetch status and timestamp for one workspace target index. */
export interface TargetIndexMeta {
  fetchedAt: number;
  status: "ok" | "partial";
}

/** The labels and rule classes needed by the target tree and search. */
export interface TargetIndex {
  meta: TargetIndexMeta;
  targets: Target[];
}

/** Relationships loaded only after a target is selected. */
export interface TargetDetail {
  target: Target;
  edges: Edge[];
  files: FileEntry[];
  status: "ok" | "partial";
}

/** Output paths from `bazel cquery`, capped so huge filegroups stay cheap. */
export interface TargetOutputs {
  paths: string[];
  truncated: boolean;
}

/** A key/value pair carried by an action, such as an environment variable. */
export interface ActionKeyValue {
  key: string;
  value: string;
}

/** Public metadata of the configuration an action runs under. */
export interface ActionConfiguration {
  mnemonic: string;
  platformName: string;
  checksum: string;
  isTool: boolean;
}

/** One action from `bazel aquery`, with its command line and file sets. */
export interface TargetAction {
  output: string;
  mnemonic: string;
  owner: string;
  arguments: string[];
  inputs: string[];
  inputsTruncated: boolean;
  outputs: string[];
  environmentVariables: ActionKeyValue[];
  configuration: ActionConfiguration | null;
  executionPlatform: string;
}

export interface TargetActions {
  actions: TargetAction[];
  truncated: boolean;
}

/** The selected inspector tab. `definition` is static; the other two run
 * configured-target reads under the applied build flags. */
export type TargetTab = "definition" | "configured" | "actions";

/** The configuration selected by `cquery` for this target. */
export interface ConfigSummary {
  mnemonic: string;
  checksum: string;
}

/** One direct dependency edge of the configured target. `transition` is a
 * short badge such as `exec`; null for edges that keep the parent's
 * configuration or have none. */
export interface ConfiguredDep {
  label: string;
  transition: string | null;
}

/** Everything the Configured tab needs: the target's configuration summary,
 * its `cquery --output=files` outputs, and direct dependency edges. */
export interface ConfiguredTarget {
  configuration: ConfigSummary | null;
  outputs: TargetOutputs;
  deps: ConfiguredDep[];
  depsTruncated: boolean;
}

/** The tagged result of one inspector phase read: exactly one key names the
 * phase that produced it. */
export type TargetPhaseData = { configured: ConfiguredTarget } | { actions: TargetActions };

/** One inspector read; each step runs one Bazel command. */
export type TargetReadStep = "configuration" | "transitions" | "outputs" | "actions";

/** A user-facing summary for a failed core operation. */
export interface IngestError {
  exitCode: number | null;
  summary: string;
}

/** Target patterns included in discovery. */
export interface TargetScope {
  include: string[];
}

/** Per-workspace state inside the aggregate snapshot. */
export interface WorkspaceState {
  workspace: string;
  scope: TargetScope;
}

/** The result of one target-list refresh: the parsed index, or the error
 * that prevented it. A partial fetch carries both. */
export interface TargetLoad {
  snapshot: TargetIndex | null;
  error: IngestError | null;
}

/** One Bazel operation Zelith launched; presence in `invocations` means the
 * core still owns its worker, including the final output drain. */
export interface OwnedInvocation {
  id: string;
  workspace: string;
  cmd: string;
  pattern: string;
  startedAt: number;
  stoppable: boolean;
  stopping: boolean;
  console: boolean;
}

/** The local facts of one released terminal invocation whose build-event
 * record is missing or unreadable. `processExitCode` is the Bazel process's
 * own exit, never a BEP exit code, `output` is its bounded terminal tail,
 * and the entry is never persisted. */
export interface ProcessDiagnostic {
  id: string;
  workspace: string;
  cmd: string;
  pattern: string;
  startedAt: number;
  processExitCode: number | null;
  output: string;
}

/** The workspace-scoped history reply: saved build-event records, or the
 * store failure that kept them from loading, plus the workspace's local
 * diagnostics for terminal completions without a readable record. */
export type InvocationHistory = {
  saved: { Ok: InvocationRecord[] } | { Err: string };
  diagnostics: ProcessDiagnostic[];
};

/** The aggregate core snapshot pushed on every change; `revision` grows
 * monotonically so stale pushes cannot overwrite newer state. */
export interface AppState {
  revision: number;
  besError: string | null;
  /** The actual bound endpoint, or null if the receiver cannot accept invocations. */
  besEndpoint: string | null;
  bazel: string | null;
  lastWorkspace: string | null;
  workspaces: WorkspaceState[];
  invocations: OwnedInvocation[];
}

/** `open_workspace` reply: the canonical id plus the fresh aggregate state. */
export interface OpenWorkspaceResult {
  workspace: string;
  state: AppState;
}

/** Commands accepted by the command composer. */
export type BazelCommand = "build" | "test" | "run";

/** An invocation and the facts observed in its build events. */
export interface InvocationRecord {
  id: string;
  workspace?: string;
  cmd?: string;
  pattern?: string;
  startedAt?: number;
  finishedAt?: number;
  exitCode?: number;
}

/** A section result that distinguishes missing, partial, and failed data. */
export type Section<T> =
  | { status: "ok"; data: T }
  | { status: "incomplete"; data: T; note: string }
  | { status: "missing" }
  | { status: "parse_error" | "read_error" | "not_available"; note: string };

/** One `bazel info` key/value pair reported for a workspace. */
export interface BazelInfoEntry {
  key: string;
  value: string;
}

/** Release version and every `bazel info` entry for one workspace. */
export interface WorkspaceInfo {
  version: string | null;
  entries: BazelInfoEntry[];
}

/** Per-runner action execution counts from BEP `BuildMetrics.action_summary`. */
export interface RunnerCount {
  name: string;
  count: number;
}

/** Parsed build-event data retained for inspection in the invocation view. */
export interface Bep {
  targets: Array<{ label: string; ok: boolean; aspect?: string; configuration?: string }>;
  tests: Array<{ label: string; status: string }>;
  originalCommandLine?: string;
  canonicalCommandLine?: string;
  options?: {
    startupOptions: string[];
    explicitStartupOptions: string[];
    cmdLine: string[];
    explicitCmdLine: string[];
  };
  runnerCounts?: RunnerCount[];
  truncated: boolean;
}

/** Raw trace events stay intact; timeline geometry is a separate projection. */
export type TraceEvent = Record<string, unknown>;

/** Timing data returned for an invocation, before layout and viewport projection. */
export interface Profile {
  phases: Array<{ name: string; startUs: number; endUs: number }>;
  lanes: Array<{ tid: number; name: string; events: TraceEvent[] }>;
}

/** Saved detail sections associated with an invocation record. */
export interface InvocationDetail {
  record: InvocationRecord;
  log: Section<string>;
  bep: Section<Bep>;
}
