import type { RunRecord, Section } from "./types.ts";

export type RunStatus = {
  kind: "succeeded" | "failed" | "running" | "stopping" | "unknown";
  label: string;
};

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

export function sectionData<T>(section: Section<T> | undefined): T | null {
  return section && "data" in section ? section.data : null;
}

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
