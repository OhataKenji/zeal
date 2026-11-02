import type { AnalyticsFilter, DailyAnalytics } from "../lib/analytics.ts";
import type { InvocationRecord } from "../lib/types.ts";

type AnalyticsRecord = Pick<InvocationRecord, "exitCode" | "finishedAt"> & {
  cmd: string;
  pattern: string;
  startedAt: number;
};
type ResolvedFilter = Partial<AnalyticsFilter> & { start: string; end: string };
type Outcome = "ok" | "failed" | "interrupted";
interface DaySamples {
  count: number;
  ok: number;
  failed: number;
  interrupted: number;
  durations: number[];
}

function dayString(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

// Calendar validation uses UTC so dates skipped by a local timezone remain valid.
function utcDay(value: string): number | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) return null;
  const date = new Date(0);
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  date.setUTCFullYear(year, month - 1, day);
  return date.getUTCFullYear() === year &&
    date.getUTCMonth() === month - 1 &&
    date.getUTCDate() === day
    ? date.getTime()
    : null;
}

function resolveFilter(requested: Partial<AnalyticsFilter>, now: number) {
  const from = new Date(now);
  from.setDate(from.getDate() - 29);
  const filter: ResolvedFilter = {
    ...requested,
    start: requested.start ?? dayString(from),
    end: requested.end ?? dayString(new Date(now)),
  };
  const first = utcDay(filter.start);
  const last = utcDay(filter.end);
  if (first === null || last === null) throw new Error("invalid analytics filter");
  const spanDays = (last - first) / 86400000 + 1;
  // Mirror the backend contract: reject, never silently truncate.
  if (
    !(spanDays >= 1 && spanDays <= 366) ||
    (filter.cmd != null && !["build", "test"].includes(filter.cmd)) ||
    (filter.outcome != null && !["ok", "failed", "interrupted"].includes(filter.outcome)) ||
    (filter.pattern != null && new TextEncoder().encode(filter.pattern).length > 64 * 1024)
  ) {
    throw new Error("invalid analytics filter");
  }
  return { filter, first, spanDays };
}

function outcome(inv: AnalyticsRecord): Outcome {
  if (inv.exitCode === 0) return "ok";
  return inv.exitCode === 8 ? "interrupted" : "failed";
}

function matchesPeriod(inv: AnalyticsRecord, filter: ResolvedFilter): boolean {
  const day = dayString(new Date(inv.startedAt));
  return (
    inv.exitCode !== undefined &&
    ["build", "test"].includes(inv.cmd) &&
    day >= filter.start &&
    day <= filter.end &&
    (filter.cmd == null || inv.cmd === filter.cmd) &&
    (filter.outcome == null || outcome(inv) === filter.outcome)
  );
}

function duration(inv: AnalyticsRecord): number | null {
  return inv.finishedAt === undefined ? null : Math.max(0, inv.finishedAt - inv.startedAt);
}

function timing(durations: readonly number[]) {
  const sorted = [...durations].sort((a, b) => a - b);
  return {
    totalMs: sorted.reduce((a, b) => a + b, 0),
    p50Ms: sorted.length ? sorted[Math.ceil(sorted.length * 0.5) - 1] : 0,
    p95Ms: sorted.length ? sorted[Math.ceil((sorted.length * 95) / 100) - 1] : 0,
  };
}

function emptyDays(first: number, spanDays: number): Map<string, DaySamples> {
  const days = new Map<string, DaySamples>();
  for (let index = 0; index < spanDays; index++) {
    const date = new Date(first + index * 86400000);
    const day = `${String(date.getUTCFullYear()).padStart(4, "0")}-${String(date.getUTCMonth() + 1).padStart(2, "0")}-${String(date.getUTCDate()).padStart(2, "0")}`;
    days.set(day, { count: 0, ok: 0, failed: 0, interrupted: 0, durations: [] });
  }
  return days;
}

function recordInvocation(
  days: Map<string, DaySamples>,
  inv: AnalyticsRecord,
  filter: ResolvedFilter,
) {
  if (filter.pattern != null && inv.pattern !== filter.pattern) return;
  const slot = days.get(dayString(new Date(inv.startedAt)));
  if (!slot) throw new Error("Preview analytics day was not initialized.");
  slot.count += 1;
  slot[outcome(inv)] += 1;
  const elapsed = duration(inv);
  if (elapsed !== null) slot.durations.push(elapsed);
}

function dailyAnalytics(
  invocations: readonly AnalyticsRecord[],
  filter: ResolvedFilter,
  first: number,
  spanDays: number,
): DailyAnalytics["days"] {
  const days = emptyDays(first, spanDays);
  for (const inv of invocations) {
    recordInvocation(days, inv, filter);
  }
  return [...days].map(([day, slot]) => ({
    day,
    count: slot.count,
    timed: slot.durations.length,
    ok: slot.ok,
    failed: slot.failed,
    interrupted: slot.interrupted,
    ...timing(slot.durations),
  }));
}

function targetAnalytics(invocations: readonly AnalyticsRecord[]): DailyAnalytics["targets"] {
  const targets = new Map<string, { invocations: number; durations: number[] }>();
  for (const inv of invocations) {
    const slot = targets.get(inv.pattern) ?? { invocations: 0, durations: [] };
    slot.invocations += 1;
    const elapsed = duration(inv);
    if (elapsed !== null) slot.durations.push(elapsed);
    targets.set(inv.pattern, slot);
  }
  return [...targets]
    .map(([pattern, slot]) => ({
      pattern,
      invocations: slot.invocations,
      ...timing(slot.durations),
    }))
    .sort(
      (a, b) =>
        b.totalMs - a.totalMs || (a.pattern < b.pattern ? -1 : a.pattern > b.pattern ? 1 : 0),
    )
    .slice(0, 10);
}

/** Computes preview analytics without mutating records or reading the system clock. */
export function previewAnalytics(
  records: readonly AnalyticsRecord[],
  requested: Partial<AnalyticsFilter>,
  now: number,
): Omit<DailyAnalytics, "filter"> & { filter: ResolvedFilter } {
  const { filter, first, spanDays } = resolveFilter(requested, now);
  const invocations = records.filter((inv) => matchesPeriod(inv, filter));
  return {
    filter,
    today: dayString(new Date(now)),
    days: dailyAnalytics(invocations, filter, first, spanDays),
    // The ranking intentionally ignores the selected target.
    targets: targetAnalytics(invocations),
  };
}
