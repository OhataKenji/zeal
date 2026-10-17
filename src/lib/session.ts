import type { AppState, RunDetail, RunRecord } from "./types.ts";

export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, callback: (payload: T) => void): Promise<() => void>;
}

export interface SessionView {
  state: AppState | null;
  ready: boolean;
  opening: boolean;
  error: string | null;
  runs: RunRecord[];
  runsError: string | null;
  selectedRun: string | null;
  detail: RunDetail | null;
  detailLoading: boolean;
  detailError: string | null;
}

export function emptyView(): SessionView {
  return {
    state: null,
    ready: false,
    opening: false,
    error: null,
    runs: [],
    runsError: null,
    selectedRun: null,
    detail: null,
    detailLoading: false,
    detailError: null,
  };
}

// One session owns request generations and subscriptions. Core owns the runs.
export function createSession(transport: Transport, changed: (view: SessionView) => void) {
  let view = emptyView();
  let disposed = false;
  let stateRevision = 0;
  let listGeneration = 0;
  let detailGeneration = 0;
  const subscriptions: (() => void)[] = [];

  function update(patch: Partial<SessionView>) {
    if (disposed) return;
    view = { ...view, ...patch };
    changed(view);
  }

  async function loadRuns() {
    const generation = ++listGeneration;
    if (!view.state?.workspace) return;
    try {
      const runs = await transport.invoke<RunRecord[]>("list_runs");
      if (generation === listGeneration) {
        update({ runs, runsError: null });
      }
    } catch (error) {
      if (generation === listGeneration) {
        update({ runsError: String(error) });
      }
    }
  }

  async function loadDetail() {
    const id = view.selectedRun;
    const generation = ++detailGeneration;
    // Do not parse observations still being written.
    if (!id || !view.ready || id === view.state?.running) return;
    update({ detailLoading: true, detailError: null });
    try {
      const detail = await transport.invoke<RunDetail>("get_run", { id });
      if (generation === detailGeneration) {
        update({ detail, detailLoading: false });
      }
    } catch (error) {
      if (generation === detailGeneration) {
        update({ detailError: String(error), detailLoading: false });
      }
    }
  }

  function selectRun(id: string | null) {
    update({ selectedRun: id, detail: null, detailError: null, detailLoading: false });
    void loadDetail();
  }

  function acceptState(state: AppState) {
    if (disposed) return;
    const before = view.state;
    const resumed = !view.ready;
    ++stateRevision;
    if (state.workspace !== before?.workspace) {
      ++listGeneration;
      ++detailGeneration;
      update({
        runs: [],
        runsError: null,
        selectedRun: null,
        detail: null,
        detailLoading: false,
        detailError: null,
      });
    }
    update({ state, ready: true, ...(resumed ? { error: null } : {}) });
    if (resumed || state.workspace !== before?.workspace || state.running !== before?.running) {
      void loadRuns();
    }
    if (resumed || (before?.running && before.running !== state.running)) void loadDetail();
  }

  async function subscribe<T>(event: string, callback: (payload: T) => void) {
    const unlisten = await transport.listen(event, callback);
    if (disposed) unlisten();
    else subscriptions.push(unlisten);
  }

  function dispose() {
    disposed = true;
    subscriptions.splice(0).forEach((unlisten) => unlisten());
  }

  async function init() {
    try {
      await subscribe<AppState>("app-state", acceptState);
      if (disposed) return;
      const revision = stateRevision;
      const state = await transport.invoke<AppState>("get_state");
      if (revision === stateRevision) acceptState(state);
    } catch (error) {
      update({ error: `接続できません: ${String(error)}` });
      dispose();
    }
  }

  async function openWorkspace(path: string) {
    if (!view.ready || view.opening || view.state?.running || view.state?.ingest === "running") {
      throw new Error("Bazel の処理が終わってから workspace を変更してください");
    }
    update({ opening: true });
    const revision = stateRevision;
    try {
      const state = await transport.invoke<AppState>("open_workspace", { path });
      if (revision === stateRevision) acceptState(state);
    } finally {
      update({ opening: false });
    }
  }

  async function refresh() {
    try {
      await transport.invoke("refresh");
    } catch (error) {
      update({ error: String(error) });
    }
  }

  return { init, dispose, openWorkspace, refresh, selectRun, loadDetail, loadRuns };
}
