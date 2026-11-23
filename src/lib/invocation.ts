import { targetError } from "./target.ts";
import type {
  BazelCommand,
  InvocationRecord,
  OwnedInvocation,
  ProcessDiagnostic,
  Section,
} from "./types.ts";

/** Display status and label for one invocation record. */
export interface InvocationStatus {
  kind: "succeeded" | "failed" | "interrupted" | "running" | "stopping" | "unknown";
  label: string;
}

/** A diagnostic's record never carries a BEP exit code. */
export interface InvocationRow {
  record: InvocationRecord;
  execution?: OwnedInvocation;
  diagnostic?: ProcessDiagnostic;
}

export type HistoryFilter = "all" | "failed" | "running";

export function rowStatus({ record, execution, diagnostic }: InvocationRow): InvocationStatus {
  return invocationStatus(record, execution, diagnostic?.processExitCode);
}

export function matchesHistoryFilter(row: InvocationRow, filter: HistoryFilter): boolean {
  if (filter === "all") return true;
  const { kind } = rowStatus(row);
  return filter === "running" ? kind === "running" || kind === "stopping" : kind === "failed";
}

/** Labels a finished record: a `run` reports its build's BEP result, never
 * the program's exit. */
function bepStatus(record: InvocationRecord): InvocationStatus {
  const subject = record.cmd === "run" ? "build " : "";
  if (record.exitCode === 8) return { kind: "interrupted", label: `■ ${subject}interrupted` };
  return record.exitCode === 0
    ? { kind: "succeeded", label: `✓ ${subject}succeeded` }
    : { kind: "failed", label: `✗ ${subject}failed (exit ${record.exitCode})` };
}

/** Resolves an invocation's result: a live owned entry always reads as
 * running/stopping (a `run` program may outlive its successful build), then
 * the BEP exit code. Without BEP a nonzero local process exit can explain
 * failure or interruption, but cannot tell us the build succeeded. */
export function invocationStatus(
  record: InvocationRecord | undefined,
  execution?: OwnedInvocation,
  processExitCode?: number | null,
): InvocationStatus {
  if (execution) {
    return execution.stopping
      ? { kind: "stopping", label: "■ stopping" }
      : { kind: "running", label: "● running" };
  }
  if (record?.exitCode !== undefined) return bepStatus(record);
  // A run can forward its program's own exit code, including 8.
  if (processExitCode === 8 && (record?.cmd === "build" || record?.cmd === "test")) {
    return { kind: "interrupted", label: "■ process interrupted" };
  }
  if (processExitCode != null && processExitCode !== 0) {
    return { kind: "failed", label: `✗ process failed (exit ${processExitCode})` };
  }
  return { kind: "unknown", label: "? outcome unknown" };
}

/** Returns section data for successful or incomplete reads. */
export function sectionData<T>(section: Section<T> | undefined): T | null {
  return section && "data" in section ? section.data : null;
}

/** Chooses a notice tone without treating unavailable records as failures. */
export function sectionNoticeTone(section: Section<unknown>): "error" | "warning" | "neutral" {
  switch (section.status) {
    case "parse_error":
    case "read_error":
      return "error";
    case "incomplete":
      return "warning";
    case "ok":
    case "missing":
    case "not_available":
      return "neutral";
  }
}

/** Summarizes the status of an invocation detail section. */
export function sectionMessage(section: Section<unknown>): string {
  switch (section.status) {
    case "ok":
      return "ok";
    case "incomplete":
      return `incomplete — ${section.note}`;
    case "missing":
      return "missing — no record";
    case "parse_error":
      return `parse error — ${section.note}`;
    case "read_error":
      return `read error — ${section.note}`;
    case "not_available":
      return `not available — ${section.note}`;
  }
}

/** Extracts a valid `(cmd, pattern)` pair when a finished invocation can be safely re-run. */
export function rerunArgs(
  record: InvocationRecord | undefined,
): { cmd: BazelCommand; pattern: string } | null {
  if (!record?.cmd || !record.pattern) return null;
  if (record.cmd !== "build" && record.cmd !== "test" && record.cmd !== "run") return null;
  if (targetError(record.pattern) !== null) return null;
  return { cmd: record.cmd, pattern: record.pattern };
}
