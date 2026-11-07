import DiffMatchPatch from "diff-match-patch";
import type { InvocationDetail, InvocationRecord } from "./types.ts";
import { quoteShellArgument } from "./invocation/options.ts";

export type ComparisonInvocation = Pick<InvocationDetail, "record" | "bep">;

export interface ComparisonPart {
  text: string;
  changed: boolean;
}

export interface ComparisonRow {
  label: string;
  left: string;
  right: string;
  leftParts: ComparisonPart[];
  rightParts: ComparisonPart[];
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

const dmp = new DiffMatchPatch();

function plainParts(text: string): ComparisonPart[] {
  return [{ text, changed: false }];
}

function diffParts(
  left: string,
  right: string,
  status: ComparisonRow["status"],
  leftEmpty: boolean,
  rightEmpty: boolean,
): { leftParts: ComparisonPart[]; rightParts: ComparisonPart[] } {
  const plain = { leftParts: plainParts(left), rightParts: plainParts(right) };
  if (status !== "changed" || left === right) return plain;
  if (leftEmpty || rightEmpty) {
    return {
      leftParts: leftEmpty ? plainParts(left) : [{ text: left, changed: true }],
      rightParts: rightEmpty ? plainParts(right) : [{ text: right, changed: true }],
    };
  }
  const diffs = dmp.diff_main(left, right);
  if (diffs.length === 0) return plain;
  dmp.diff_cleanupSemantic(diffs);
  if (diffs.some(([, text]) => /[\uD800-\uDBFF]$|^[\uDC00-\uDFFF]/.test(text))) {
    return {
      leftParts: [{ text: left, changed: true }],
      rightParts: [{ text: right, changed: true }],
    };
  }
  return {
    leftParts: diffs.filter(([op]) => op !== 1).map(([op, text]) => ({ text, changed: op === -1 })),
    rightParts: diffs
      .filter(([op]) => op !== -1)
      .map(([op, text]) => ({ text, changed: op === 1 })),
  };
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
  const leftText = display(leftValue, format);
  const rightText = display(rightValue, format);
  const status: ComparisonRow["status"] =
    leftValue !== null && rightValue !== null && leftValue !== undefined && rightValue !== undefined
      ? sameValue(leftValue, rightValue)
        ? "same"
        : "changed"
      : "unknown";
  const leftEmpty = leftValue === "" || (Array.isArray(leftValue) && leftValue.length === 0);
  const rightEmpty = rightValue === "" || (Array.isArray(rightValue) && rightValue.length === 0);
  const { leftParts, rightParts } = diffParts(leftText, rightText, status, leftEmpty, rightEmpty);
  return { label, left: leftText, right: rightText, leftParts, rightParts, status };
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
