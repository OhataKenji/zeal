import type { InvocationDetail, InvocationRecord } from "./types.ts";
import { quoteShellArgument } from "./invocation/options.ts";

export type ComparisonInvocation = Pick<InvocationDetail, "record" | "bep">;

export interface ComparisonRow {
  label: string;
  left: string;
  right: string;
  status: "same" | "changed" | "unknown";
}

type Value = string | number | string[] | undefined;
type Cell = Value | null;
type Facet = (invocation: ComparisonInvocation) => Value;
type Format = (value: number) => string;

const MISSING = "Not recorded";
const NO_OPTIONS = "No options";
const EMPTY = "—";

function duration(record: InvocationRecord | undefined): number | undefined {
  if (record?.startedAt === undefined || record.finishedAt === undefined) return undefined;
  const elapsed = record.finishedAt - record.startedAt;
  return Number.isFinite(elapsed) && elapsed >= 0 ? elapsed : undefined;
}

function sameValue(left: Value, right: Value): boolean {
  if (Array.isArray(left) && Array.isArray(right)) {
    return left.length === right.length && left.every((value, index) => value === right[index]);
  }
  return left === right;
}

function bepOptions(invocation: ComparisonInvocation) {
  return invocation.bep.status === "ok" ? invocation.bep.data.options : undefined;
}

function display(value: Cell, format?: Format): string {
  if (value === null) return EMPTY;
  if (value === undefined) return MISSING;
  if (Array.isArray(value)) {
    return value.length === 0 ? NO_OPTIONS : value.map(quoteShellArgument).join("\n");
  }
  if (typeof value === "number") return format === undefined ? String(value) : format(value);
  return value;
}

function row(
  label: string,
  facet: Facet,
  left: ComparisonInvocation | null,
  right: ComparisonInvocation | null,
  format?: Format,
): ComparisonRow {
  const leftValue: Cell = left === null ? null : facet(left);
  const rightValue: Cell = right === null ? null : facet(right);
  return {
    label,
    left: display(leftValue, format),
    right: display(rightValue, format),
    status:
      leftValue !== null &&
      rightValue !== null &&
      leftValue !== undefined &&
      rightValue !== undefined
        ? sameValue(leftValue, rightValue)
          ? "same"
          : "changed"
        : "unknown",
  };
}

const formatDate = (value: number) => new Date(value).toLocaleString();
const formatDuration = (value: number) => `${(value / 1000).toFixed(3)}s`;

export function comparisonRows(
  left: ComparisonInvocation | null,
  right: ComparisonInvocation | null,
): ComparisonRow[] {
  return [
    row("Invocation ID", (invocation) => invocation.record.id, left, right),
    row("Command", (invocation) => invocation.record.cmd, left, right),
    row("Pattern", (invocation) => invocation.record.pattern, left, right),
    row("Exit code", (invocation) => invocation.record.exitCode, left, right),
    row("Start date", (invocation) => invocation.record.startedAt, left, right, formatDate),
    row("Duration", (invocation) => duration(invocation.record), left, right, formatDuration),
    row(
      "Explicit command line",
      (invocation) => bepOptions(invocation)?.explicitCmdLine,
      left,
      right,
    ),
    row("Full command line", (invocation) => bepOptions(invocation)?.cmdLine, left, right),
    row(
      "Explicit startup options",
      (invocation) => bepOptions(invocation)?.explicitStartupOptions,
      left,
      right,
    ),
    row(
      "Full startup options",
      (invocation) => bepOptions(invocation)?.startupOptions,
      left,
      right,
    ),
  ];
}
