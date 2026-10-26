import { targetError } from "./target.ts";
import type { AppState, Profile, RunCmd, RunDetail, RunRecord, Section } from "./types.ts";

export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, callback: (payload: T) => void): Promise<() => void>;
}

export interface SessionView {
  console: RunRecord | null;
  consoleOpen: boolean;
  state: AppState | null;
  ready: boolean;
  starting: boolean;
  opening: boolean;
  error: string | null;
  runs: RunRecord[];
  runsError: string | null;
  selectedRun: string | null;
  initialRunTab: "All" | "Logs";
  detail: RunDetail | null;
  detailLoading: boolean;
  detailError: string | null;
  profile: Section<Profile> | null;
  profileLoading: boolean;
  profileError: string | null;
}

export function emptyView(): SessionView {
  return {
    console: null,
    consoleOpen: false,
    state: null,
    ready: false,
    starting: false,
    opening: false,
    error: null,
    runs: [],
    runsError: null,
    selectedRun: null,
    initialRunTab: "All",
    detail: null,
    detailLoading: false,
    detailError: null,
    profile: null,
    profileLoading: false,
    profileError: null,
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
  let listing = false;
  let detailGeneration = 0;
  let profileGeneration = 0;
  let unlisten: (() => void) | undefined;
  function update(patch: Partial<SessionView>) {
    if (disposed) return;
    view = { ...view, ...patch };
    // Reconcile start_run first: a short command can finish before its reply.
    // Each Console gets one transition; later updates never restart it.
    if (
      view.console &&
      view.consoleOpen &&
      !view.opening &&
      view.ready &&
      !view.starting &&
      !view.state?.running
    ) {
      selectRun(view.console.id, "All");
      return;
    }
    changed(view);
  }

  async function loadRuns() {
    if (listing || !view.state?.workspace || disposed) return;
    listing = true;
    const generation = ++listGeneration;
    try {
      // `runs` keeps the live record: its row is the way back to the Console.
      const runs = await transport.invoke<RunRecord[]>("list_runs");
      if (generation === listGeneration) {
        if (JSON.stringify(runs) !== JSON.stringify(view.runs) || view.runsError)
          update({ runs, runsError: null });
      }
    } catch (error) {
      if (generation === listGeneration) {
        update({ runsError: String(error) });
      }
    } finally {
      listing = false;
    }
  }

  async function loadDetail() {
    const id = view.selectedRun;
    // Saved observations are read after the active run ends.
    if (!id || !view.ready || view.starting || view.detailLoading || id === view.state?.running)
      return;
    const generation = ++detailGeneration;
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

  async function loadProfile() {
    const id = view.selectedRun;
    if (!id || !view.ready || view.starting || view.profileLoading || id === view.state?.running)
      return;
    const generation = ++profileGeneration;
    update({ profileLoading: true, profileError: null });
    try {
      const profile = await transport.invoke<Section<Profile>>("get_run_profile", { id });
      if (generation === profileGeneration) {
        update({ profile, profileLoading: false });
      }
    } catch (error) {
      if (generation === profileGeneration) {
        update({ profileError: String(error), profileLoading: false });
      }
    }
  }

  function loadSelected() {
    void loadDetail();
    void loadProfile();
  }

  function selectRun(id: string | null, initialRunTab: "All" | "Logs" = "All") {
    ++detailGeneration;
    ++profileGeneration;
    update({
      consoleOpen: false,
      selectedRun: id,
      detail: null,
      initialRunTab,
      detailError: null,
      detailLoading: false,
      profile: null,
      profileLoading: false,
      profileError: null,
    });
    loadSelected();
  }

  // Reopen the live run's Console; anything else falls through to detail.
  function openConsole(record: RunRecord) {
    if (record.id !== view.state?.running) {
      selectRun(record.id);
      return;
    }
    if (view.consoleOpen && view.console?.id === record.id) return;
    ++detailGeneration;
    ++profileGeneration;
    update({
      console: record,
      consoleOpen: true,
      selectedRun: null,
      detail: null,
      detailError: null,
      detailLoading: false,
      profile: null,
      profileLoading: false,
      profileError: null,
    });
  }

  function acceptState(state: AppState) {
    if (disposed) return;
    const before = view.state;
    const resumed = !view.ready;
    ++stateRevision;
    if (state.workspace !== before?.workspace) {
      ++listGeneration;
      ++detailGeneration;
      ++profileGeneration;
      update({
        console: null,
        consoleOpen: false,
        runs: [],
        runsError: null,
        selectedRun: null,
        detail: null,
        detailLoading: false,
        detailError: null,
        profile: null,
        profileLoading: false,
        profileError: null,
      });
    }
    update({
      state,
      ready: true,
      ...(resumed ? { error: null } : {}),
    });
    if (resumed || state.workspace !== before?.workspace || state.running !== before?.running) {
      void loadRuns();
    }
    if (resumed || (before?.running && before.running !== state.running)) loadSelected();
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
    update({ starting: true, error: null });
    try {
      // The target goes through verbatim: one argument, no shell interpretation.
      const record = await transport.invoke<RunRecord>("start_run", { cmd, target });
      if (disposed || workspace !== view.state?.workspace) return;
      selectRun(null);
      update({ console: record, consoleOpen: true });
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
      if (!disposed && workspace === view.state?.workspace && view.ready) loadSelected();
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

  return {
    init,
    dispose,
    start,
    stop,
    openWorkspace,
    refresh,
    selectRun,
    openConsole,
    loadDetail,
    loadProfile,
    loadRuns,
  };
}
