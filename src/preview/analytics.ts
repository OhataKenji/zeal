import type { AnalyticsFilter, DailyAnalytics } from "../lib/analytics.ts";
import type { RunRecord } from "../lib/types.ts";

type AnalyticsRecord = Pick<RunRecord, "exitCode" | "finishedAt"> & {
  cmd: string;
  target: string;
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
    (filter.target != null && new TextEncoder().encode(filter.target).length > 64 * 1024)
  ) {
    throw new Error("invalid analytics filter");
  }
  return { filter, first, spanDays };
}

function outcome(run: AnalyticsRecord): Outcome {
  if (run.exitCode === 0) return "ok";
  return run.exitCode === 8 ? "interrupted" : "failed";
}

function matchesPeriod(run: AnalyticsRecord, filter: ResolvedFilter): boolean {
  const day = dayString(new Date(run.startedAt));
  return (
    run.exitCode !== undefined &&
    ["build", "test"].includes(run.cmd) &&
    day >= filter.start &&
    day <= filter.end &&
    (filter.cmd == null || run.cmd === filter.cmd) &&
    (filter.outcome == null || outcome(run) === filter.outcome)
  );
}

function duration(run: AnalyticsRecord): number | null {
  return run.finishedAt === undefined ? null : Math.max(0, run.finishedAt - run.startedAt);
}

function timing(durations: readonly number[]) {
  const sorted = [...durations].sort((a, b) => a - b);
  return {
    totalMs: sorted.reduce((a, b) => a + b, 0),
    p50Ms: sorted.length ? sorted[Math.ceil(sorted.length * 0.5) - 1] : 0,
    p95Ms: sorted.length ? sorted[Math.ceil((sorted.length * 95) / 100) - 1] : 0,
  };
}

function dailyAnalytics(
  runs: readonly AnalyticsRecord[],
  filter: ResolvedFilter,
  first: number,
  spanDays: number,
): DailyAnalytics["days"] {
  const days = new Map<string, DaySamples>();
  for (let index = 0; index < spanDays; index++) {
    const date = new Date(first + index * 86400000);
    const day = `${String(date.getUTCFullYear()).padStart(4, "0")}-${String(date.getUTCMonth() + 1).padStart(2, "0")}-${String(date.getUTCDate()).padStart(2, "0")}`;
    days.set(day, { count: 0, ok: 0, failed: 0, interrupted: 0, durations: [] });
  }
  for (const run of runs) {
    if (filter.target != null && run.target !== filter.target) continue;
    const slot = days.get(dayString(new Date(run.startedAt)));
    if (!slot) throw new Error("Preview analytics day was not initialized.");
    slot.count += 1;
    slot[outcome(run)] += 1;
    const elapsed = duration(run);
    if (elapsed !== null) slot.durations.push(elapsed);
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

function targetAnalytics(runs: readonly AnalyticsRecord[]): DailyAnalytics["targets"] {
  const targets = new Map<string, { runs: number; durations: number[] }>();
  for (const run of runs) {
    const slot = targets.get(run.target) ?? { runs: 0, durations: [] };
    slot.runs += 1;
    const elapsed = duration(run);
    if (elapsed !== null) slot.durations.push(elapsed);
    targets.set(run.target, slot);
  }
  return [...targets]
    .map(([target, slot]) => ({ target, runs: slot.runs, ...timing(slot.durations) }))
    .sort(
      (a, b) => b.totalMs - a.totalMs || (a.target < b.target ? -1 : a.target > b.target ? 1 : 0),
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
  const runs = records.filter((run) => matchesPeriod(run, filter));
  return {
    filter,
    today: dayString(new Date(now)),
    days: dailyAnalytics(runs, filter, first, spanDays),
    // The ranking intentionally ignores the selected target.
    targets: targetAnalytics(runs),
  };
}
