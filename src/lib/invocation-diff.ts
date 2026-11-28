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

interface CacheStats {
  total: number;
  cached: number;
  hitPct: number | null;
}

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

function isEmptyCell(value: Cell): boolean {
  return value === "" || (Array.isArray(value) && value.length === 0);
}

function rowStatus(leftValue: Cell, rightValue: Cell): ComparisonRow["status"] {
  if (leftValue == null || rightValue == null) return "unknown";
  return sameValue(leftValue, rightValue) ? "same" : "changed";
}

function row(
  label: string,
  facet: Facet,
  baseline: ComparisonInvocation | null,
  current: ComparisonInvocation | null,
  options: { format?: Format; inlineDiff?: boolean } = {},
): ComparisonRow {
  const leftValue: Cell = baseline === null ? null : facet(baseline);
  const rightValue: Cell = current === null ? null : facet(current);
  const leftText = display(leftValue, options.format);
  const rightText = display(rightValue, options.format);
  const status = rowStatus(leftValue, rightValue);
  const { leftParts, rightParts } =
    options.inlineDiff === false
      ? { leftParts: plainParts(leftText), rightParts: plainParts(rightText) }
      : diffParts(leftText, rightText, status, isEmptyCell(leftValue), isEmptyCell(rightValue));
  return { label, left: leftText, right: rightText, leftParts, rightParts, status };
}

const formatDate = (value: number) => new Date(value).toLocaleString();
const formatDuration = (value: number) => `${(value / 1000).toFixed(3)}s`;

function formatSignedInt(value: number): string {
  return value > 0 ? `+${value}` : String(value);
}

function formatDurationDelta(deltaMs: number): string {
  if (deltaMs === 0) return "0.000s";
  return `${deltaMs > 0 ? "+" : "-"}${(Math.abs(deltaMs) / 1000).toFixed(3)}s`;
}

function formatPctDelta(deltaPct: number): string {
  if (deltaPct === 0) return "0 pp";
  return `${deltaPct > 0 ? "+" : ""}${deltaPct} pp`;
}

function deltaTone(delta: number, higherIsBetter: boolean): ComparisonDelta["tone"] {
  if (delta === 0) return "same";
  return delta > 0 === higherIsBetter ? "better" : "worse";
}

function cacheStats(invocation: ComparisonInvocation): CacheStats | undefined {
  if (invocation.bep.status !== "ok") return undefined;
  const summary: RunnerSummary | null = summarizeRunners(invocation.bep.data.runnerCounts);
  if (summary === null) return { total: 0, cached: 0, hitPct: null };
  return { total: summary.total, cached: summary.cached, hitPct: summary.hitPct };
}

function formatCacheCell(stats: CacheStats | null | undefined): string {
  if (stats === null) return EMPTY;
  if (stats === undefined) return MISSING;
  if (stats.total === 0) return "0 spawns";
  return `${stats.hitPct}% (${stats.cached}/${stats.total})`;
}

function sameCache(left: CacheStats, right: CacheStats): boolean {
  return left.total === right.total && left.cached === right.cached && left.hitPct === right.hitPct;
}

function cacheDelta(baseline: CacheStats, current: CacheStats): ComparisonDelta {
  const spawnDiff = current.total - baseline.total;
  if (baseline.hitPct === null || current.hitPct === null) {
    return {
      text: spawnDiff === 0 ? "0 spawns" : `${formatSignedInt(spawnDiff)} spawns`,
      tone: "same",
    };
  }
  const pctDiff = current.hitPct - baseline.hitPct;
  if (spawnDiff === 0) {
    return { text: formatPctDelta(pctDiff), tone: deltaTone(pctDiff, true) };
  }
  return {
    text: `${formatPctDelta(pctDiff)} (${formatSignedInt(spawnDiff)} spawns)`,
    tone: "same",
  };
}

function durationRow(
  baseline: ComparisonInvocation | null,
  current: ComparisonInvocation | null,
): ComparisonRow {
  const base = row("Duration", (invocation) => duration(invocation.record), baseline, current, {
    format: formatDuration,
    inlineDiff: false,
  });
  const baseMs = baseline ? duration(baseline.record) : undefined;
  const curMs = current ? duration(current.record) : undefined;
  if (baseMs === undefined || curMs === undefined) return base;
  const deltaMs = curMs - baseMs;
  return {
    ...base,
    delta: { text: formatDurationDelta(deltaMs), tone: deltaTone(deltaMs, false) },
  };
}

function spawnCacheRow(
  baseline: ComparisonInvocation | null,
  current: ComparisonInvocation | null,
): ComparisonRow {
  const leftStats = baseline === null ? null : cacheStats(baseline);
  const rightStats = current === null ? null : cacheStats(current);
  const leftText = formatCacheCell(leftStats);
  const rightText = formatCacheCell(rightStats);
  const bothKnown = leftStats != null && rightStats != null;
  const status: ComparisonRow["status"] = bothKnown
    ? sameCache(leftStats, rightStats)
      ? "same"
      : "changed"
    : "unknown";
  return {
    label: "Spawn cache hits",
    left: leftText,
    right: rightText,
    leftParts: plainParts(leftText),
    rightParts: plainParts(rightText),
    status,
    ...(bothKnown ? { delta: cacheDelta(leftStats, rightStats) } : {}),
  };
}

/** Normalizes whitespace-separated target patterns so token order differences still match. */
export function normalizePattern(pattern: string | undefined): string {
  if (!pattern) return "";
  return pattern.trim().split(/\s+/).filter(Boolean).sort().join(" ");
}

function sameTargetKey(current: InvocationRecord, candidate: InvocationRecord): boolean {
  const curPattern = normalizePattern(current.pattern);
  if (!curPattern || normalizePattern(candidate.pattern) !== curPattern) return false;
  return current.cmd === undefined || candidate.cmd === current.cmd;
}

function pickLatest(records: readonly InvocationRecord[]): InvocationRecord | null {
  return records.reduce<InvocationRecord | null>(
    (best, candidate) =>
      best === null || candidate.startedAt! > best.startedAt! ? candidate : best,
    null,
  );
}

/** Finds the latest earlier settled successful invocation matching pattern/cmd, preferring same workspace. */
export function previousSuccessfulInvocation(
  current: InvocationRecord,
  history: readonly InvocationRecord[],
  runningIds: readonly string[],
): InvocationRecord | null {
  if (!normalizePattern(current.pattern) || current.startedAt === undefined) return null;
  const running = new Set(runningIds);
  const matches = history.filter(
    (entry) =>
      entry.id !== current.id &&
      !running.has(entry.id) &&
      entry.exitCode === 0 &&
      entry.startedAt !== undefined &&
      entry.startedAt < current.startedAt! &&
      sameTargetKey(current, entry),
  );
  const sameWorkspace = matches.filter(
    (entry) => current.workspace !== undefined && entry.workspace === current.workspace,
  );
  return pickLatest(sameWorkspace.length > 0 ? sameWorkspace : matches);
}

/** Splits settled history invocations into same-target and other candidates for the baseline picker. */
export function recentComparableInvocations(
  current: InvocationRecord,
  history: readonly InvocationRecord[],
  runningIds: readonly string[],
): { sameTarget: InvocationRecord[]; other: InvocationRecord[] } {
  const running = new Set(runningIds);
  const sameTarget: InvocationRecord[] = [];
  const other: InvocationRecord[] = [];
  for (const entry of history) {
    if (entry.id === current.id || running.has(entry.id)) continue;
    (sameTargetKey(current, entry) ? sameTarget : other).push(entry);
  }
  return { sameTarget, other };
}

/** Formats a compact one-line label for an invocation in the baseline picker. */
export function formatInvocationOption(
  record: InvocationRecord,
  isPreviousSuccess = false,
): string {
  const icon = record.exitCode === 0 ? "✓" : record.exitCode === undefined ? "?" : "✗";
  const head = isPreviousSuccess
    ? `Previous successful (${icon})`
    : `${icon} ${[record.cmd, record.pattern].filter(Boolean).join(" ") || record.id.slice(0, 8)}`;
  const parts = [head];
  const ms = duration(record);
  if (ms !== undefined) parts.push(formatDuration(ms));
  if (record.startedAt !== undefined) parts.push(formatDate(record.startedAt));
  return parts.join(" · ");
}

/** Builds comparison rows ordered left = baseline, right = current. */
export function comparisonRows(
  baseline: ComparisonInvocation | null,
  current: ComparisonInvocation | null,
): ComparisonRow[] {
  return [
    durationRow(baseline, current),
    spawnCacheRow(baseline, current),
    row("Exit code", (invocation) => invocation.record.exitCode, baseline, current, {
      inlineDiff: false,
    }),
    row("Command", (invocation) => invocation.record.cmd, baseline, current),
    row("Pattern", (invocation) => invocation.record.pattern, baseline, current),
    row("Workspace", (invocation) => invocation.record.workspace, baseline, current),
    row(
      "Explicit command line",
      (invocation) => bepOptions(invocation)?.explicitCmdLine,
      baseline,
      current,
    ),
    row("Full command line", (invocation) => bepOptions(invocation)?.cmdLine, baseline, current),
    row(
      "Explicit startup options",
      (invocation) => bepOptions(invocation)?.explicitStartupOptions,
      baseline,
      current,
    ),
    row(
      "Full startup options",
      (invocation) => bepOptions(invocation)?.startupOptions,
      baseline,
      current,
    ),
    row("Start date", (invocation) => invocation.record.startedAt, baseline, current, {
      format: formatDate,
      inlineDiff: false,
    }),
    row("Invocation ID", (invocation) => invocation.record.id, baseline, current, {
      inlineDiff: false,
    }),
  ];
}
