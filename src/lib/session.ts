import type { AppState, RunDetail, RunLog, RunRecord } from "./types.ts";

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
  liveLog: string;
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
    liveLog: "",
  };
}

export function canSend(view: SessionView, target: string): boolean {
  return !!(
    view.ready &&
    !view.starting &&
    !view.opening &&
    view.state?.workspace &&
    view.state.bazel &&
    view.state.ingest === "idle" &&
    !view.state.running &&
    target.trim()
  );
}

// One session owns request generations and subscriptions. Core remains the
// owner of runs; this is only the current view and live, not-yet-read output.
export function createSession(
  transport: Transport,
  changed: (view: SessionView) => void,
  schedule: (callback: () => void) => () => void,
) {
  let view = emptyView();
  let disposed = false;
  let stateRevision = 0;
  let listGeneration = 0;
  let detailGeneration = 0;
  const subscriptions: (() => void)[] = [];
  let output: { id: string; lines: string[] } | null = null;
  let cancelLogFrame: (() => void) | undefined;

  function update(patch: Partial<SessionView>) {
    if (disposed) return;
    view = { ...view, ...patch };
    changed(view);
  }

  function showLog() {
    cancelLogFrame = undefined;
    update({
      liveLog: output?.id === view.selectedRun ? output.lines.join("\n") : "",
    });
  }

  function releaseHiddenLogs() {
    // There is one active run. Keep early output until start_run returns its id.
    if (view.starting) return;
    if (
      view.ready &&
      output &&
      output.id !== view.state?.running &&
      output.id !== view.selectedRun
    ) {
      output = null;
    }
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
    // Do not parse observations still being written. Completed reads replace
    // live output, never append to it (run-log has no offset for deduplication).
    if (!id || !view.ready || view.starting || id === view.state?.running) return;
    update({ detailLoading: true, detailError: null });
    try {
      const detail = await transport.invoke<RunDetail>("get_run", { id });
      if (generation === detailGeneration) {
        update({ detail, detailLoading: false, liveLog: "" });
        if (output?.id === id) output = null;
      }
    } catch (error) {
      if (generation === detailGeneration) {
        update({ detailError: String(error), detailLoading: false });
      }
    }
  }

  function selectRun(id: string | null) {
    update({ selectedRun: id, detail: null, detailError: null, detailLoading: false });
    releaseHiddenLogs();
    showLog();
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
      output = null;
      update({
        runs: [],
        runsError: null,
        selectedRun: null,
        detail: null,
        detailLoading: false,
        detailError: null,
        liveLog: "",
      });
    }
    update({ state, ready: true, ...(resumed ? { error: null } : {}) });
    releaseHiddenLogs();
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
    cancelLogFrame?.();
    subscriptions.splice(0).forEach((unlisten) => unlisten());
    output = null;
  }

  async function init() {
    try {
      // Log first: no Send can happen until both subscriptions are ready.
      await subscribe<RunLog>("run-log", ({ id, line }) => {
        if (disposed) return;
        if (output?.id !== id) output = { id, lines: [] };
        output.lines.push(line);
        if (!cancelLogFrame) cancelLogFrame = schedule(showLog);
      });
      if (disposed) return;
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

  async function start(cmd: "build" | "test", target: string) {
    if (!canSend(view, target)) return;
    const workspace = view.state?.workspace;
    update({ starting: true, error: null });
    try {
      const record = await transport.invoke<RunRecord>("start_run", { cmd, target: target.trim() });
      if (disposed || workspace !== view.state?.workspace) return;
      update({ runs: [record, ...view.runs.filter((run) => run.id !== record.id)] });
      selectRun(record.id);
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
      releaseHiddenLogs();
      if (!disposed && workspace === view.state?.workspace && view.ready) void loadDetail();
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
    try {
      await transport.invoke("refresh");
    } catch (error) {
      update({ error: String(error) });
    }
  }

  return { init, dispose, start, openWorkspace, refresh, selectRun, loadDetail, loadRuns };
}
