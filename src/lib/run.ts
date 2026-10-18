import type { RunRecord, Section } from "./types.ts";

export function exitStatus(code: number): string {
  return code === -1 ? "signal" : `exit ${code}`;
}

export function outcome(record: RunRecord): string {
  if (record.outcome === "ok") return "✓ succeeded";
  const result = record.outcome === "interrupted" ? "■ interrupted" : "✗ failed";
  return `${result} (${exitStatus(record.exitCode)})`;
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
