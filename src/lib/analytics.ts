import type { Transport } from "./session.ts";
import type { RunCmd, RunRecord } from "./types.ts";

export interface AnalyticsFilter {
  start: string | null;
  end: string | null;
  cmd: RunCmd | null;
  outcome: "ok" | "failed" | "interrupted" | null;
  day: string | null;
}
export interface DailyAnalytics {
  filter: AnalyticsFilter;
  today: string;
  days: { day: string; count: number }[];
  total: number;
  runs: RunRecord[];
}
export interface AnalyticsView {
  filter: AnalyticsFilter;
  data: DailyAnalytics | null;
  loading: boolean;
  error: string | null;
}
export function emptyAnalytics(): AnalyticsView {
  return {
    filter: { start: null, end: null, cmd: null, outcome: null, day: null },
    data: null,
    loading: false,
    error: null,
  };
}

// Lifetime is one workspace view. A request generation protects both filters
// and drill-down against late responses; disposal protects workspace switches.
export function createAnalytics(
  transport: Pick<Transport, "invoke">,
  changed: (view: AnalyticsView) => void,
) {
  let view = emptyAnalytics();
  let generation = 0;
  let disposed = false;
  async function load(filter = view.filter) {
    const request = ++generation;
    view = { filter: { ...filter }, data: null, loading: true, error: null };
    changed(view);
    try {
      const data = await transport.invoke<DailyAnalytics>("get_analytics", { filter: view.filter });
      if (disposed || request !== generation) return;
      view = { filter: data.filter, data, loading: false, error: null };
    } catch (error) {
      if (disposed || request !== generation) return;
      view = { ...view, loading: false, error: String(error) };
    }
    changed(view);
  }
  return {
    load,
    filter: (filter: AnalyticsFilter) => load({ ...filter, day: null }),
    selectDay: (day: string | null) => load({ ...view.filter, day }),
    dispose: () => {
      disposed = true;
      ++generation;
    },
  };
}
