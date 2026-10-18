export interface TargetLocation {
  file: string;
  line: number;
}

export interface Target {
  label: string;
  pkg: string;
  name: string;
  ruleClass: string;
  location: TargetLocation | null;
}

export interface Edge {
  from: string;
  to: string;
  attr: string;
}

export interface FileEntry {
  path: string;
  owner: string;
  role: string;
}

export interface SnapshotMeta {
  fetchedAt: number;
  status: "ok" | "partial";
}

export interface Snapshot {
  meta: SnapshotMeta;
  targets: Target[];
  edges: Edge[];
  files: FileEntry[];
}

export interface IngestError {
  exitCode: number | null;
  summary: string;
}

export interface AppState {
  telemetryEnabled?: boolean;
  ingestOperationId?: string | null;
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

// The commands the core accepts from the composer.
export type RunCmd = "build" | "test";

export interface RunRecord {
  id: string;
  workspace: string;
  cmd: string;
  target: string;
  startedAt: number;
  finishedAt?: number;
  exitCode?: number;
}

export type Section<T> =
  | { status: "ok"; data: T }
  | { status: "incomplete"; data: T; note: string }
  | { status: "missing" }
  | { status: "parse_error" | "read_error" | "not_available"; note: string };

export interface Bep {
  targets: { label: string; ok: boolean }[];
  tests: { label: string; status: string }[];
  truncated: boolean;
}

// Raw fields stay intact for inspection; geometry is a separate projection.
export type TraceEvent = Record<string, unknown>;

export interface Profile {
  phases: { name: string; startUs: number; endUs: number }[];
  lanes: { tid: number; name: string; events: TraceEvent[] }[];
}

export interface RunDetail {
  record: RunRecord;
  log: Section<string>;
  bep: Section<Bep>;
  profile: Section<Profile>;
}
