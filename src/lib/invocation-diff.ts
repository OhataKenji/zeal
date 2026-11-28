import DiffMatchPatch from "diff-match-patch";
import type { InvocationDetail, InvocationRecord } from "./types.ts";
import { summarizeRunners } from "./invocation.ts";
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
type TestItem = { label: string; status: string };

const MISSING = "Not recorded";
const NO_OPTIONS = "No options";
const EMPTY = "—";
const FAILED_TESTS = new Set(["FAILED", "TIMEOUT", "FAILED_TO_BUILD", "REMOTE_FAILURE"]);
const dmp = new DiffMatchPatch();

const plain = (text: string): ComparisonPart[] => [{ text, changed: false }];
const signed = (n: number) => (n > 0 ? `+${n}` : String(n));
const formatDate = (value: number) => new Date(value).toLocaleString();
const formatDuration = (value: number) => `${(value / 1000).toFixed(3)}s`;
const bepData = (inv: ComparisonInvocation | null) =>
  inv === null ? null : inv.bep.status === "ok" ? inv.bep.data : undefined;

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

function diffParts(
  left: string,
  right: string,
  status: ComparisonRow["status"],
  leftEmpty: boolean,
  rightEmpty: boolean,
): { leftParts: ComparisonPart[]; rightParts: ComparisonPart[] } {
  const unchanged = { leftParts: plain(left), rightParts: plain(right) };
  if (status !== "changed" || left === right) return unchanged;
  if (leftEmpty || rightEmpty) {
    return {
      leftParts: leftEmpty ? plain(left) : [{ text: left, changed: true }],
      rightParts: rightEmpty ? plain(right) : [{ text: right, changed: true }],
    };
  }
  const diffs = dmp.diff_main(left, right);
  if (diffs.length === 0) return unchanged;
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
  return typeof value === "number" && format ? format(value) : String(value);
}

function plainRow(
  label: string,
  left: string,
  right: string,
  status: ComparisonRow["status"],
): ComparisonRow {
  return { label, left, right, leftParts: plain(left), rightParts: plain(right), status };
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
  const left = display(leftValue, options.format);
  const right = display(rightValue, options.format);
  const status: ComparisonRow["status"] =
    leftValue == null || rightValue == null
      ? "unknown"
      : sameValue(leftValue, rightValue)
        ? "same"
        : "changed";
  if (options.inlineDiff === false) return plainRow(label, left, right, status);
  const isEmpty = (v: Cell) => v === "" || (Array.isArray(v) && v.length === 0);
  const parts = diffParts(left, right, status, isEmpty(leftValue), isEmpty(rightValue));
  return { label, left, right, ...parts, status };
}

function durationRow(
  baseline: ComparisonInvocation | null,
  current: ComparisonInvocation | null,
): ComparisonRow {
  const baseMs = baseline === null ? null : duration(baseline.record);
  const curMs = current === null ? null : duration(current.record);
  const left = display(baseMs, formatDuration);
  const rightBase = display(curMs, formatDuration);
  if (baseMs == null || curMs == null) return plainRow("Duration", left, rightBase, "unknown");
  const deltaMs = curMs - baseMs;
  const right =
    deltaMs === 0
      ? rightBase
      : `${rightBase} (${deltaMs > 0 ? "+" : "-"}${formatDuration(Math.abs(deltaMs))})`;
  return plainRow("Duration", left, right, deltaMs === 0 ? "same" : "changed");
}

function cacheStats(inv: ComparisonInvocation | null) {
  const data = bepData(inv);
  if (data === null) return null;
  if (data?.runnerCounts === undefined) return undefined;
  const summary = summarizeRunners(data.runnerCounts);
  return summary
    ? { total: summary.total, cached: summary.cached, hitPct: summary.hitPct }
    : { total: 0, cached: 0, hitPct: null as number | null };
}

function spawnCacheRow(
  baseline: ComparisonInvocation | null,
  current: ComparisonInvocation | null,
): ComparisonRow {
  const left = cacheStats(baseline);
  const right = cacheStats(current);
  const fmt = (s: ReturnType<typeof cacheStats>) =>
    s === null
      ? EMPTY
      : s === undefined
        ? MISSING
        : s.total === 0
          ? "0 spawns"
          : `${s.hitPct}% (${s.cached}/${s.total})`;
  if (left == null || right == null) {
    return plainRow("Spawn cache hits", fmt(left), fmt(right), "unknown");
  }
  const same =
    left.total === right.total && left.cached === right.cached && left.hitPct === right.hitPct;
  if (same) return plainRow("Spawn cache hits", fmt(left), fmt(right), "same");
  const spawnDiff = right.total - left.total;
  const delta =
    left.hitPct === null || right.hitPct === null
      ? spawnDiff === 0
        ? ""
        : ` (${signed(spawnDiff)} spawns)`
      : spawnDiff === 0
        ? ` (${signed(right.hitPct - left.hitPct)} pp)`
        : ` (${signed(right.hitPct - left.hitPct)} pp, ${signed(spawnDiff)} spawns)`;
  return plainRow("Spawn cache hits", fmt(left), `${fmt(right)}${delta}`, "changed");
}

function exitCodeRow(
  baseline: ComparisonInvocation | null,
  current: ComparisonInvocation | null,
): ComparisonRow {
  const baseCode = baseline === null ? null : baseline.record.exitCode;
  const curCode = current === null ? null : current.record.exitCode;
  const left = display(baseCode);
  const rightBase = display(curCode);
  if (baseCode == null || curCode == null) return plainRow("Exit code", left, rightBase, "unknown");
  if (baseCode === curCode) return plainRow("Exit code", left, rightBase, "same");
  const tag =
    curCode === 8
      ? " (Interrupted)"
      : baseCode === 0
        ? " (Failed)"
        : curCode === 0 && baseCode !== 8
          ? " (Fixed)"
          : "";
  return plainRow("Exit code", left, `${rightBase}${tag}`, "changed");
}

const isFailedTest = (status: string | undefined) =>
  status !== undefined && FAILED_TESTS.has(status);

function summarizeTests(tests: readonly TestItem[] | null | undefined): string {
  if (tests === null) return EMPTY;
  if (tests === undefined) return MISSING;
  if (tests.length === 0) return "No tests";
  const passed = tests.filter((t) => t.status === "PASSED").length;
  const failed = tests.filter((t) => isFailedTest(t.status)).length;
  const flaky = tests.filter((t) => t.status === "FLAKY").length;
  const incomplete = tests.length - passed - failed - flaky;
  const parts = [
    ...(failed ? [`${failed} failed`] : []),
    ...(flaky ? [`${flaky} flaky`] : []),
    ...(incomplete ? [`${incomplete} incomplete`] : []),
  ];
  return parts.length > 0
    ? `${passed}/${tests.length} passed (${parts.join(", ")})`
    : `${passed}/${tests.length} passed`;
}

function formatTestDiff(baseline: readonly TestItem[], current: readonly TestItem[]) {
  const baseMap = new Map(baseline.map((t) => [t.label, t.status]));
  const curMap = new Map(current.map((t) => [t.label, t.status]));
  const newlyFailed = current
    .filter((t) => isFailedTest(t.status) && !isFailedTest(baseMap.get(t.label)))
    .map((t) => t.label);
  const fixed = baseline.filter(
    (t) => isFailedTest(t.status) && curMap.get(t.label) === "PASSED",
  ).length;
  const deltaParts = [
    ...(newlyFailed.length ? [`+${newlyFailed.length} failed`] : []),
    ...(fixed ? [`${fixed} fixed`] : []),
  ];
  const suffix = deltaParts.length ? ` [${deltaParts.join(", ")}]` : "";
  const extra = newlyFailed.length > 5 ? `\n(+${newlyFailed.length - 5} more)` : "";
  const failList = newlyFailed.length
    ? `\nNew failures:\n${newlyFailed.slice(0, 5).join("\n")}${extra}`
    : "";
  return { changed: newlyFailed.length > 0 || fixed > 0, suffix: `${suffix}${failList}` };
}

function testsRow(
  baseline: ComparisonInvocation | null,
  current: ComparisonInvocation | null,
): ComparisonRow | null {
  const baseTests = baseline === null ? null : bepData(baseline)?.tests;
  const curTests = current === null ? null : bepData(current)?.tests;
  if (!baseTests?.length && !curTests?.length) return null;
  const left = summarizeTests(baseTests);
  const rightBase = summarizeTests(curTests);
  if (baseTests == null || curTests == null) return plainRow("Tests", left, rightBase, "unknown");
  const diff = formatTestDiff(baseTests, curTests);
  return plainRow(
    "Tests",
    left,
    `${rightBase}${diff.suffix}`,
    left === rightBase && !diff.changed ? "same" : "changed",
  );
}

/** Normalizes whitespace-separated target patterns; preserves order when exclusions (`-...`) exist. */
export function normalizePattern(pattern: string | undefined): string {
  if (!pattern) return "";
  const tokens = pattern.trim().split(/\s+/).filter(Boolean);
  return tokens.some((token) => token.startsWith("-")) ? tokens.join(" ") : tokens.sort().join(" ");
}

function sameTargetKey(current: InvocationRecord, candidate: InvocationRecord): boolean {
  const curPattern = normalizePattern(current.pattern);
  if (!curPattern || normalizePattern(candidate.pattern) !== curPattern) return false;
  return current.cmd === undefined || candidate.cmd === current.cmd;
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
  const sameWs = matches.filter(
    (entry) => current.workspace !== undefined && entry.workspace === current.workspace,
  );
  const pool = sameWs.length > 0 ? sameWs : matches;
  return pool.reduce<InvocationRecord | null>(
    (best, c) => (best === null || c.startedAt! > best.startedAt! ? c : best),
    null,
  );
}

/** Splits settled history invocations into same-target (previous successful first) and other candidates. */
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
  const prev = previousSuccessfulInvocation(current, history, runningIds);
  const idx = prev ? sameTarget.findIndex((entry) => entry.id === prev.id) : -1;
  if (idx > 0) sameTarget.unshift(...sameTarget.splice(idx, 1));
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
  const ms = duration(record);
  const date =
    record.startedAt !== undefined
      ? new Date(record.startedAt).toLocaleString(undefined, {
          month: "numeric",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        })
      : undefined;
  return [head, ms !== undefined ? formatDuration(ms) : undefined, date]
    .filter(Boolean)
    .join(" · ");
}

/** Builds comparison rows ordered left = baseline, right = current. */
export function comparisonRows(
  baseline: ComparisonInvocation | null,
  current: ComparisonInvocation | null,
): ComparisonRow[] {
  const tests = testsRow(baseline, current);
  const opt = (inv: ComparisonInvocation) => bepData(inv)?.options;
  return [
    durationRow(baseline, current),
    spawnCacheRow(baseline, current),
    exitCodeRow(baseline, current),
    ...(tests ? [tests] : []),
    row("Command", (inv) => inv.record.cmd, baseline, current),
    row("Pattern", (inv) => inv.record.pattern, baseline, current),
    row("Workspace", (inv) => inv.record.workspace, baseline, current),
    row("Explicit command line", (inv) => opt(inv)?.explicitCmdLine, baseline, current),
    row("Full command line", (inv) => opt(inv)?.cmdLine, baseline, current),
    row("Explicit startup options", (inv) => opt(inv)?.explicitStartupOptions, baseline, current),
    row("Full startup options", (inv) => opt(inv)?.startupOptions, baseline, current),
    row("Start date", (inv) => inv.record.startedAt, baseline, current, {
      format: formatDate,
      inlineDiff: false,
    }),
    row("Invocation ID", (inv) => inv.record.id, baseline, current, { inlineDiff: false }),
  ];
}
