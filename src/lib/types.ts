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
  workspace: string | null;
  bazel: string | null;
  ingest: "idle" | "running";
  snapshot: Snapshot | null;
  lastError: IngestError | null;
}
