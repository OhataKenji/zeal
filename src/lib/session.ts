import { targetError } from "./target.ts";
import type { AppState, RunCmd, RunDetail, RunRecord } from "./types.ts";

export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, callback: (payload: T) => void): Promise<() => void>;
}

export interface SessionView {
  state: AppState | null;
  ready: boolean;
  starting: boolean;
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
    starting: false,
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

// Manual input does not depend on a snapshot: after a failed ingestion the
// composer stays usable once ingestion is idle.
export function canSend(view: SessionView, target: string): boolean {
  return !!(
    view.ready &&
    !view.starting &&
    !view.opening &&
    view.state?.workspace &&
    view.state.bazel &&
    view.state.ingest === "idle" &&
    !view.state.running &&
    targetError(target) === null
  );
}

// Core owns stop and may release an unconfirmed run with an explicit note.
export function canStop(view: SessionView): boolean {
  return !!(view.ready && view.state?.running && !view.state.stopping);
}

// One session owns request generations and subscriptions. Core remains the
// owner of runs; this is only the current view.
export function createSession(transport: Transport, changed: (view: SessionView) => void) {
  let view = emptyView();
  let disposed = false;
  let stateRevision = 0;
  let listGeneration = 0;
  let detailGeneration = 0;
  let detailInFlight = false;
  let followSession = false;
  let unlisten: (() => void) | undefined;

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
    // Logs and observations are read after the run leaves the active slot.
    if (!id || !view.ready || view.starting) return;
    if (detailInFlight) {
      update({
        detailLoading: false,
        detailError: "別の詳細を読み込み中です。完了後に Retry を押してください",
      });
      return;
    }
    detailInFlight = true;
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
    } finally {
      detailInFlight = false;
    }
  }

  function selectRun(id: string | null, follow = false) {
    if (!follow) followSession = false;
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
    if (resumed && state.running) followSession = true;
    update({ state, ready: true, ...(resumed ? { error: null } : {}) });
    if (resumed || state.workspace !== before?.workspace || state.running !== before?.running) {
      void loadRuns();
    }
    if (!state.running && (before?.running || view.starting) && followSession) {
      if (state.completedRuns.length > 0) selectRun(state.completedRuns[0], true);
    } else if (resumed) void loadDetail();
  }

  function dispose() {
    disposed = true;
    unlisten?.();
    unlisten = undefined;
  }

  async function init() {
    try {
      const remove = await transport.listen<AppState>("app-state", acceptState);
      if (disposed) {
        remove();
        return;
      }
      unlisten = remove;
      const revision = stateRevision;
      const state = await transport.invoke<AppState>("get_state");
      if (revision === stateRevision) acceptState(state);
    } catch (error) {
      update({ error: `接続できません: ${String(error)}` });
      dispose();
    }
  }

  async function start(cmd: RunCmd, target: string) {
    if (!canSend(view, target)) return;
    const workspace = view.state?.workspace;
    selectRun(null);
    followSession = true;
    update({ starting: true, error: null });
    try {
      // The target goes through verbatim: one argument, no shell interpretation.
      await transport.invoke<string>("start_run", { cmd, target });
      if (disposed || workspace !== view.state?.workspace) return;
      // Invoke responses and events use different Tauri delivery paths. The
      // reply may precede its running event, so reconcile before deciding to
      // read completed files. Events received during this read take priority.
      const revision = stateRevision;
      try {
        const state = await transport.invoke<AppState>("get_state");
        if (disposed || workspace !== view.state?.workspace) return;
        if (revision === stateRevision) acceptState(state);
      } catch (error) {
        if (workspace === view.state?.workspace && revision === stateRevision) {
          update({
            ready: false,
            error: `実行は開始済みです。状態の取得に失敗しました。次の通知を待っています: ${String(error)}`,
          });
        }
      }
      // The run may already have finished before start_run's reply. Never
      // write that reply back into the authoritative running state.
      void loadRuns();
    } catch (error) {
      if (workspace === view.state?.workspace) update({ error: String(error) });
    } finally {
      update({ starting: false });
      if (!disposed && workspace === view.state?.workspace && view.ready) void loadDetail();
    }
  }

  async function stop() {
    if (!canStop(view)) return;
    const workspace = view.state?.workspace;
    try {
      await transport.invoke("stop_run");
    } catch (error) {
      if (workspace === view.state?.workspace) update({ error: String(error) });
    }
  }

  async function openWorkspace(path: string) {
    if (
      !view.ready ||
      view.opening ||
      view.starting ||
      view.state?.running ||
      view.state?.ingest === "running"
    ) {
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
    if (
      !view.ready ||
      view.starting ||
      view.opening ||
      view.state?.running ||
      view.state?.ingest === "running"
    )
      return;
    try {
      await transport.invoke("refresh");
    } catch (error) {
      update({ error: String(error) });
    }
  }

  return { init, dispose, start, stop, openWorkspace, refresh, selectRun, loadDetail, loadRuns };
}
