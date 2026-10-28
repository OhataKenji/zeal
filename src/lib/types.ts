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

/** Fetch status and timestamp for one workspace snapshot. */
export interface SnapshotMeta {
  fetchedAt: number;
  status: "ok" | "partial";
}

/** The core's current index of workspace targets, edges, and files. */
export interface Snapshot {
  meta: SnapshotMeta;
  targets: Target[];
  edges: Edge[];
  files: FileEntry[];
}

/** A user-facing summary for a failed core operation. */
export interface IngestError {
  exitCode: number | null;
  summary: string;
}

/** Current core state. A workspace switch clears workspace-scoped data. */
export interface AppState {
  besError?: string | null;
  workspace: string | null;
  bazel: string | null;
  ingest: "idle" | "running";
  snapshot: Snapshot | null;
  lastError: IngestError | null;
  running: string | null;
  // Stop is best-effort. An unconfirmed exit releases the slot with runNote.
  stopping: boolean;
  runNote: string | null;
}

/** Commands accepted by the run composer. */
export type RunCmd = "build" | "test" | "run";

/** A persisted or active run record, keyed by its core-generated ID. */
export interface RunRecord {
  external?: boolean;
  id: string;
  workspace: string;
  cmd: string;
  target: string;
  startedAt: number;
  finishedAt?: number;
  exitCode?: number;
}

/** A section result that distinguishes missing, partial, and failed data. */
export type Section<T> =
  | { status: "ok"; data: T }
  | { status: "incomplete"; data: T; note: string }
  | { status: "missing" }
  | { status: "parse_error" | "read_error" | "not_available"; note: string };

/** Parsed build-event data retained for inspection in the run view. */
export interface Bep {
  targets: Array<{ label: string; ok: boolean }>;
  tests: Array<{ label: string; status: string }>;
  options?: {
    startupOptions: string[];
    explicitStartupOptions: string[];
    cmdLine: string[];
    explicitCmdLine: string[];
  };
  truncated: boolean;
}

/** Raw trace events stay intact; timeline geometry is a separate projection. */
export type TraceEvent = Record<string, unknown>;

/** Timing data returned for a run, before layout and viewport projection. */
export interface Profile {
  phases: Array<{ name: string; startUs: number; endUs: number }>;
  lanes: Array<{ tid: number; name: string; events: TraceEvent[] }>;
}

/** Saved detail sections associated with a run record. */
export interface RunDetail {
  record: RunRecord;
  log: Section<string>;
  bep: Section<Bep>;
}
