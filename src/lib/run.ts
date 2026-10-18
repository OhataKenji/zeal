import type { RunRecord, Section } from "./types.ts";

export function outcome(record: RunRecord, running: string | null, stopping = false): string {
  if (record.finishedAt !== undefined && record.exitCode !== undefined) {
    return record.exitCode === 0 ? "✓ succeeded" : `✗ failed (exit ${record.exitCode})`;
  }
  if (record.id === running) return stopping ? "■ stopping" : "● running";
  return "? outcome unknown";
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
      return "missing — 記録がありません";
    case "parse_error":
      return `parse error — ${section.note}`;
    case "read_error":
      return `read error — ${section.note}`;
    case "not_available":
      return `not available — ${section.note}`;
  }
}
