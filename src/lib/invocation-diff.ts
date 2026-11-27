import DiffMatchPatch from "diff-match-patch";
import type { InvocationDetail, InvocationRecord } from "./types.ts";
import { summarizeRunners, type RunnerSummary } from "./invocation.ts";
import { quoteShellArgument } from "./invocation/options.ts";

export type ComparisonInvocation = Pick<InvocationDetail, "record" | "bep">;

export interface ComparisonPart {
  text: string;
  changed: boolean;
}

export interface ComparisonDelta {
  text: string;
  tone: "better" | "worse" | "same";
}

export interface ComparisonRow {
  label: string;
  left: string;
  right: string;
  leftParts: ComparisonPart[];
  rightParts: ComparisonPart[];
  status: "same" | "changed" | "unknown";
  delta?: ComparisonDelta;
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

function formatDurationDelta(deltaMs: number): string {
  if (deltaMs === 0) return "0.000s";
  return `${deltaMs > 0 ? "+" : "-"}${(Math.abs(deltaMs) / 1000).toFixed(3)}s`;
}

function formatPctDelta(deltaPct: number): string {
  if (deltaPct === 0) return "0 pp";
  return `${deltaPct > 0 ? "+" : ""}${deltaPct} pp`;
}

function cacheSummary(invocation: ComparisonInvocation): RunnerSummary | undefined {
  if (invocation.bep.status !== "ok") return undefined;
  return summarizeRunners(invocation.bep.data.runnerCounts) ?? undefined;
}

function formatCacheCell(summary: RunnerSummary | null | undefined): string {
  if (summary === null) return EMPTY;
  if (summary === undefined) return MISSING;
  return `${summary.hitPct}% (${summary.cached}/${summary.total})`;
}

function sameCache(left: RunnerSummary, right: RunnerSummary): boolean {
  return left.hitPct === right.hitPct && left.cached === right.cached && left.total === right.total;
}

function deltaTone(delta: number, higherIsBetter: boolean): ComparisonDelta["tone"] {
  if (delta === 0) return "same";
  return delta > 0 === higherIsBetter ? "better" : "worse";
}

function durationRow(
  left: ComparisonInvocation | null,
  right: ComparisonInvocation | null,
): ComparisonRow {
  const base = row(
    "Duration",
    (invocation) => duration(invocation.record),
    left,
    right,
    formatDuration,
  );
  const leftMs = left ? duration(left.record) : undefined;
  const rightMs = right ? duration(right.record) : undefined;
  if (leftMs === undefined || rightMs === undefined) return base;
  const deltaMs = leftMs - rightMs;
  return {
    ...base,
    delta: { text: formatDurationDelta(deltaMs), tone: deltaTone(deltaMs, false) },
  };
}

function cacheHitRateRow(
  left: ComparisonInvocation | null,
  right: ComparisonInvocation | null,
): ComparisonRow {
  const leftCache = left === null ? null : cacheSummary(left);
  const rightCache = right === null ? null : cacheSummary(right);
  const leftText = formatCacheCell(leftCache);
  const rightText = formatCacheCell(rightCache);
  const bothKnown = leftCache != null && rightCache != null;
  const status: ComparisonRow["status"] = bothKnown
    ? sameCache(leftCache, rightCache)
      ? "same"
      : "changed"
    : "unknown";
  const { leftParts, rightParts } = diffParts(leftText, rightText, status, false, false);
  const delta = bothKnown
    ? {
        text: formatPctDelta(leftCache.hitPct - rightCache.hitPct),
        tone: deltaTone(leftCache.hitPct - rightCache.hitPct, true),
      }
    : undefined;
  return {
    label: "Cache hit rate",
    left: leftText,
    right: rightText,
    leftParts,
    rightParts,
    status,
    ...(delta ? { delta } : {}),
  };
}

function isPreviousSuccessCandidate(
  current: InvocationRecord,
  candidate: InvocationRecord,
  running: ReadonlySet<string>,
): boolean {
  if (candidate.id === current.id || running.has(candidate.id)) return false;
  if (candidate.exitCode !== 0 || candidate.pattern !== current.pattern) return false;
  if (current.cmd !== undefined && candidate.cmd !== current.cmd) return false;
  return candidate.startedAt !== undefined && candidate.startedAt < current.startedAt!;
}

/** Finds the latest settled successful invocation before `current` with the same target pattern. */
export function previousSuccessfulInvocation(
  current: InvocationRecord,
  history: readonly InvocationRecord[],
  runningIds: readonly string[],
): InvocationRecord | null {
  if (!current.pattern || current.startedAt === undefined) return null;
  const running = new Set(runningIds);
  const candidates = history.filter((entry) => isPreviousSuccessCandidate(current, entry, running));
  return candidates.reduce<InvocationRecord | null>(
    (best, candidate) =>
      best === null || candidate.startedAt! > best.startedAt! ? candidate : best,
    null,
  );
}

/** Lists settled history invocations eligible to compare against `currentId`. */
export function recentComparableInvocations(
  currentId: string,
  history: readonly InvocationRecord[],
  runningIds: readonly string[],
): InvocationRecord[] {
  return history.filter((entry) => entry.id !== currentId && !runningIds.includes(entry.id));
}

/** Formats a compact one-line label for an invocation in the comparison picker. */
export function formatInvocationOption(record: InvocationRecord): string {
  const icon = record.exitCode === 0 ? "✓" : record.exitCode === undefined ? "?" : "✗";
  const head = [record.cmd, record.pattern].filter(Boolean).join(" ") || record.id.slice(0, 8);
  const parts = [`${icon} ${head}`];
  const ms = duration(record);
  if (ms !== undefined) parts.push(formatDuration(ms));
  if (record.startedAt !== undefined) parts.push(formatDate(record.startedAt));
  return parts.join(" · ");
}

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
    durationRow(left, right),
    cacheHitRateRow(left, right),
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
