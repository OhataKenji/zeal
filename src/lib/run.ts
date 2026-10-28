import type { RunRecord, Section } from "./types.ts";

/** Display status and label for one run record. */
export interface RunStatus {
  kind: "succeeded" | "failed" | "running" | "stopping" | "unknown";
  label: string;
}

/** Resolves a run's result while preserving an unknown state for partial data. */
export function runStatus(
  record: RunRecord | undefined,
  running: string | null,
  stopping = false,
): RunStatus {
  if (record?.finishedAt !== undefined && record.exitCode !== undefined) {
    return record.exitCode === 0
      ? { kind: "succeeded", label: record.external ? "✓ build succeeded" : "✓ succeeded" }
      : {
          kind: "failed",
          label: `✗ ${record.external ? "build " : ""}failed (exit ${record.exitCode})`,
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

/** Summarizes the status of a run detail section. */
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
