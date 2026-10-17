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
  liveFromStart: boolean;
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
    liveFromStart: false,
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
  let workspaceGeneration = 0;
  let listGeneration = 0;
  let detailGeneration = 0;
  const subscriptions: (() => void)[] = [];
  const logs = new Map<string, string[]>();
  const capturedFromStart = new Set<string>();
  let cancelLogFrame: (() => void) | undefined;

  function update(patch: Partial<SessionView>) {
    if (disposed) return;
    view = { ...view, ...patch };
    changed(view);
  }

  function showLog() {
    cancelLogFrame = undefined;
    update({
      liveLog: (logs.get(view.selectedRun ?? "") ?? []).join("\n"),
      liveFromStart: capturedFromStart.has(view.selectedRun ?? ""),
    });
  }

  async function loadRuns() {
    const generation = ++listGeneration;
    const workspace = workspaceGeneration;
    if (!view.state?.workspace) return;
    try {
      const runs = await transport.invoke<RunRecord[]>("list_runs");
      if (workspace === workspaceGeneration && generation === listGeneration) {
        update({ runs, runsError: null });
      }
    } catch (error) {
      if (workspace === workspaceGeneration && generation === listGeneration) {
        update({ runsError: String(error) });
      }
    }
  }

  async function loadDetail() {
    const id = view.selectedRun;
    const generation = ++detailGeneration;
    const workspace = workspaceGeneration;
    // Do not parse observations still being written. Completed reads replace
    // live output, never append to it (run-log has no offset for deduplication).
    if (!id || id === view.state?.running) return;
    update({ detailLoading: true, detailError: null });
    try {
      const detail = await transport.invoke<RunDetail>("get_run", { id });
      if (workspace === workspaceGeneration && generation === detailGeneration) {
        update({ detail, detailLoading: false });
        logs.delete(id);
        capturedFromStart.delete(id);
      }
    } catch (error) {
      if (workspace === workspaceGeneration && generation === detailGeneration) {
        update({ detailError: String(error), detailLoading: false });
      }
    }
  }

  function selectRun(id: string | null) {
    ++detailGeneration;
    update({ selectedRun: id, detail: null, detailError: null, detailLoading: false });
    showLog();
    void loadDetail();
  }

  function acceptState(state: AppState) {
    if (disposed) return;
    const before = view.state;
    ++stateRevision;
    if (state.workspace !== before?.workspace) {
      ++workspaceGeneration;
      ++listGeneration;
      ++detailGeneration;
      logs.clear();
      capturedFromStart.clear();
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
    update({ state });
    if (state.workspace !== before?.workspace || state.running !== before?.running) {
      void loadRuns();
    }
    if (
      before?.running &&
      before.running !== state.running &&
      view.selectedRun === before.running
    ) {
      void loadDetail();
    }
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
    logs.clear();
  }

  async function init() {
    try {
      // Log first: no Send can happen until both subscriptions are ready.
      await subscribe<RunLog>("run-log", ({ id, line }) => {
        if (disposed) return;
        const lines = logs.get(id) ?? [];
        lines.push(line);
        logs.set(id, lines);
        if (!cancelLogFrame) cancelLogFrame = schedule(showLog);
      });
      if (disposed) return;
      await subscribe<AppState>("app-state", acceptState);
      if (disposed) return;
      const revision = stateRevision;
      const state = await transport.invoke<AppState>("get_state");
      if (revision === stateRevision) acceptState(state);
      update({ ready: true });
    } catch (error) {
      update({ error: `接続できません: ${String(error)}` });
      dispose();
    }
  }

  async function start(cmd: "build" | "test", target: string) {
    if (!canSend(view, target)) return;
    const workspace = workspaceGeneration;
    update({ starting: true, error: null });
    try {
      const record = await transport.invoke<RunRecord>("start_run", { cmd, target: target.trim() });
      if (disposed || workspace !== workspaceGeneration) return;
      capturedFromStart.add(record.id);
      // The run may already have finished before start_run's reply. Never
      // write that reply back into the authoritative running state.
      update({ runs: [record, ...view.runs.filter((run) => run.id !== record.id)] });
      selectRun(record.id);
      void loadRuns();
    } catch (error) {
      if (workspace === workspaceGeneration) update({ error: String(error) });
    } finally {
      update({ starting: false });
    }
  }

  async function openWorkspace(path: string) {
    if (view.opening || view.starting || view.state?.running || view.state?.ingest === "running") {
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
