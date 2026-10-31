import { COMMANDS } from "./commands.ts";
import type { Transport } from "./session.ts";

/** Date and optional run filters sent to the core analytics query. */
export interface AnalyticsFilter {
  start: string | null;
  end: string | null;
  // The store rejects every command outside build/test.
  cmd: "build" | "test" | null;
  outcome: "ok" | "failed" | "interrupted" | null;
  target: string | null;
}
/** Daily and per-target aggregates returned by the core. */
export interface DailyAnalytics {
  filter: AnalyticsFilter;
  today: string;
  days: Array<{
    day: string;
    count: number;
    timed: number;
    ok: number;
    failed: number;
    interrupted: number;
    totalMs: number;
    p50Ms: number;
    p95Ms: number;
  }>;
  targets: Array<{
    target: string;
    runs: number;
    totalMs: number;
    p50Ms: number;
    p95Ms: number;
  }>;
}
/** Query state shown by the analytics view. */
export interface AnalyticsView {
  filter: AnalyticsFilter;
  data: DailyAnalytics | null;
  loading: boolean;
  error: string | null;
}
/** Creates the empty state before the first analytics query. */
export function emptyAnalytics(): AnalyticsView {
  return {
    filter: { start: null, end: null, cmd: null, outcome: null, target: null },
    data: null,
    loading: false,
    error: null,
  };
}

/**
 * Creates a workspace-scoped analytics query. New requests supersede older
 * responses, and disposal ignores any response from the previous workspace.
 */
export function createAnalytics(
  transport: Pick<Transport, "invoke">,
  changed: (view: AnalyticsView) => void,
) {
  let view = emptyAnalytics();
  let generation = 0;
  let disposed = false;
  async function load(filter = view.filter) {
    const request = ++generation;
    // Keep the previous charts visible while the reload runs; an error
    // still drops stale data so it never sits next to an error banner.
    view = { filter: { ...filter }, data: view.data, loading: true, error: null };
    changed(view);
    try {
      const data = await transport.invoke<DailyAnalytics>(COMMANDS.getAnalytics, {
        filter: view.filter,
      });
      if (disposed || request !== generation) return;
      view = { filter: data.filter, data, loading: false, error: null };
    } catch (error) {
      if (disposed || request !== generation) return;
      view = { ...view, data: null, loading: false, error: String(error) };
    }
    changed(view);
  }
  return {
    load,
    dispose: () => {
      disposed = true;
      ++generation;
    },
  };
}

/** Formats milliseconds for chart labels, tooltips, and the results table. */
export function fmtMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const s = ms / 1000;
  // Tenths of a second under 10s; 9.95 rounds up and carries into "10s".
  const tenths = Math.round(s * 10);
  if (tenths < 100) return `${tenths / 10}s`;
  const total = Math.round(s);
  if (total < 60) return `${total}s`;
  const m = Math.floor(total / 60);
  if (m < 60) return `${m}m ${total % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** The SVG plot box every analytics chart draws into. The markup in
 * `AnalyticsView.svelte` and `segments` must agree on these numbers. */
export const PLOT = { left: 55, baseline: 200, height: 160 } as const;

/**
 * Projects samples into SVG polyline segments for the chart's fixed plot box.
 * Null samples split lines instead of joining across missing data.
 */
export function segments(values: Array<number | null>, maxValue: number, step: number): string[] {
  const out: string[] = [];
  let run: string[] = [];
  values.forEach((value, i) => {
    if (value === null) {
      if (run.length) out.push(run.join(" "));
      run = [];
      return;
    }
    run.push(
      `${PLOT.left + i * step + step / 2},${PLOT.baseline - (value / maxValue) * PLOT.height}`,
    );
  });
  if (run.length) out.push(run.join(" "));
  return out;
}
