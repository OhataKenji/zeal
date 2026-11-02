import type { InvocationRecord, Section } from "./types.ts";

/** Display status and label for one invocation record. */
export interface InvocationStatus {
  kind: "succeeded" | "failed" | "running" | "stopping" | "unknown";
  label: string;
}

/** Resolves an invocation's result while preserving an unknown state for partial data. */
export function invocationStatus(
  record: InvocationRecord | undefined,
  running: string | null,
  stopping = false,
): InvocationStatus {
  if (record?.exitCode !== undefined) {
    return record.exitCode === 0
      ? { kind: "succeeded", label: "✓ succeeded" }
      : {
          kind: "failed",
          label: `✗ failed (exit ${record.exitCode})`,
        };
  }
  if (record && record.id === running) {
    return stopping
      ? { kind: "stopping", label: "■ stopping" }
      : { kind: "running", label: "● running" };
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
