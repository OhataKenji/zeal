import { COMMANDS } from "./commands.ts";
import { targetError } from "./target.ts";
import type {
  AppState,
  Profile,
  RunCmd,
  RunDetail,
  RunRecord,
  Section,
  TargetDetail,
  TargetIndex,
  TargetScope,
} from "./types.ts";

/** IPC boundary used by a session to call the core and subscribe to events. */
export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, callback: (payload: T) => void): Promise<() => void>;
}

/** UI state for one workspace and its selected run. */
export interface SessionView {
  console: RunRecord | null;
  consoleOpen: boolean;
  state: AppState | null;
  /** Tree and search data for `state.snapshotFetchedAt`. */
  snapshot: TargetIndex | null;
  snapshotLoading: boolean;
  snapshotError: string | null;
  selectedTarget: string | null;
  targetDetail: TargetDetail | null;
  targetDetailError: string | null;
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
  profile: Section<Profile> | null;
  profileLoading: boolean;
  profileError: string | null;
}

/** Creates the empty state shown before a workspace has loaded. */
export function emptyView(): SessionView {
  return {
    console: null,
    consoleOpen: false,
    state: null,
    snapshot: null,
    snapshotLoading: false,
    snapshotError: null,
    selectedTarget: null,
    targetDetail: null,
    targetDetailError: null,
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
    profile: null,
    profileLoading: false,
    profileError: null,
  };
}

/**
 * Returns whether manual target input can start now. A loaded snapshot is not
 * required, so the composer remains usable after ingestion errors settle.
 */
export function canSend(view: SessionView, target: string): boolean {
  return !!(
    view.ready &&
    !view.starting &&
    !view.opening &&
    view.state?.workspace &&
    view.state.bazel &&
    view.state.ingest === "idle" &&
    !view.state.running &&
    view.state.query !== "running" &&
    view.state.detailQuery !== "running" &&
    targetError(target) === null
  );
}

/**
 * Returns whether the core can stop an active run that is not already stopping.
 * Core remains responsible for releasing an unconfirmed run with a note.
 */
export function canStop(view: SessionView): boolean {
  return !!(view.ready && view.state?.running && !view.state.stopping);
}

/**
 * Creates a workspace session that rejects stale requests. Call `dispose()`
 * to invalidate pending responses and remove its event subscription.
 */
export function createSession(transport: Transport, changed: (view: SessionView) => void) {
  let view = emptyView();
  let disposed = false;
  let stateRevision = 0;
  let listGeneration = 0;
  let listing = false;
  let listPending = false;
  let detailGeneration = 0;
  let profileGeneration = 0;
  let snapshotGeneration = 0;
  let snapshotRequested: number | null = null;
  let targetGeneration = 0;
  let targetRequestInFlight = false;
  let pendingStart: { workspace: string; cmd: RunCmd; target: string } | null = null;
  let unlisten: (() => void) | undefined;
  let unlistenHistory: (() => void) | undefined;
  function update(patch: Partial<SessionView>) {
    if (disposed) return;
    view = { ...view, ...patch };
    changed(view);
  }

  async function loadRuns() {
    if (!view.state?.workspace || disposed) return;
    if (listing) {
      listPending = true;
      return;
    }
    listing = true;
    const generation = ++listGeneration;
    try {
      const runs = await transport.invoke<RunRecord[]>(COMMANDS.listRuns);
      acceptLoadedRuns(generation, runs);
    } catch (error) {
      if (generation === listGeneration) {
        update({ runsError: String(error) });
      }
    } finally {
      listing = false;
      if (listPending && !disposed) {
        listPending = false;
        void loadRuns();
      }
    }
  }

  // A changed selected record means its saved observation may have completed.
  function acceptLoadedRuns(generation: number, runs: RunRecord[]) {
    if (generation !== listGeneration || disposed) return;
    if (JSON.stringify(runs) !== JSON.stringify(view.runs) || view.runsError) {
      update({ runs, runsError: null });
    }
    const selected = runs.find((run) => run.id === view.selectedRun);
    if (
      !selected ||
      !view.detail ||
      view.detailLoading ||
      view.detailError ||
      JSON.stringify(selected) === JSON.stringify(view.detail.record)
    ) {
      return;
    }
    void loadDetail();
  }

  // The DTO carries only the snapshot's fetchedAt; its data is fetched when
  // that timestamp changes, not on every state event.
  function acceptLoadedSnapshot(snapshot: TargetIndex | null) {
    snapshotRequested = null;
    update({ snapshot, snapshotLoading: false, snapshotError: null, targetDetailError: null });
    if (view.selectedTarget && !snapshot?.targets.some((t) => t.label === view.selectedTarget)) {
      selectTarget(null);
      return;
    }
    requestTargetDetail();
  }

  async function loadSnapshot() {
    // Manual calls re-check in-flight state that acceptState already guards.
    if (snapshotRequested !== null && view.state?.snapshotFetchedAt === snapshotRequested) return;
    snapshotRequested = view.state?.snapshotFetchedAt ?? null;
    const generation = ++snapshotGeneration;
    update({ snapshotLoading: true, snapshotError: null });
    try {
      const snapshot = await transport.invoke<TargetIndex | null>(COMMANDS.getSnapshot);
      if (generation === snapshotGeneration) {
        acceptLoadedSnapshot(snapshot);
      }
    } catch (error) {
      failSnapshotLoad(generation, error);
    }
  }

  function failSnapshotLoad(generation: number, error: unknown) {
    if (generation !== snapshotGeneration) return;
    snapshotRequested = null;
    update({
      snapshotLoading: false,
      snapshotError: `Could not load the workspace snapshot: ${String(error)}`,
      ...(view.selectedTarget ? { targetDetailError: "Could not load the target list" } : {}),
    });
  }

  function targetBusy(): boolean {
    const state = view.state;
    return !!(
      !view.ready ||
      view.opening ||
      view.starting ||
      !state?.workspace ||
      !state.bazel ||
      state.ingest === "running" ||
      state.running ||
      state.query === "running" ||
      state.detailQuery === "running"
    );
  }

  function requestTargetDetail() {
    const label = view.selectedTarget;
    const snapshot = view.snapshot;
    const state = view.state;
    if (
      !label ||
      disposed ||
      !snapshot ||
      view.targetDetail !== null ||
      view.targetDetailError !== null ||
      targetRequestInFlight ||
      targetBusy() ||
      snapshot.meta.fetchedAt !== state?.snapshotFetchedAt
    ) {
      return;
    }
    const generation = targetGeneration;
    const fetchedAt = snapshot.meta.fetchedAt;
    targetRequestInFlight = true;
    void (async () => {
      try {
        const detail = await transport.invoke<TargetDetail>(COMMANDS.getTargetDetail, {
          label,
          fetchedAt,
        });
        if (detail.target.label !== label) {
          throw new Error("Target detail does not match the selected label");
        }
        if (generation === targetGeneration) {
          update({ targetDetail: detail, targetDetailError: null });
        }
      } catch (error) {
        if (generation === targetGeneration) {
          update({ targetDetailError: String(error) });
        }
      } finally {
        targetRequestInFlight = false;
        if (generation !== targetGeneration) requestTargetDetail();
      }
    })();
  }

  function selectTarget(label: string | null) {
    if (label === view.selectedTarget && !view.targetDetailError) return;
    ++targetGeneration;
    update({
      selectedTarget: label,
      targetDetail: null,
      targetDetailError: null,
    });
    requestTargetDetail();
  }

  async function loadDetail() {
    const id = view.selectedRun;
    // Saved observations are read after the active run ends.
    if (!id || !view.ready || view.starting || view.detailLoading || id === view.state?.running) {
      return;
    }
    const generation = ++detailGeneration;
    update({ detailLoading: true, detailError: null });
    try {
      const detail = await transport.invoke<RunDetail>(COMMANDS.getRun, { id });
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
    if (!id || !view.ready || view.starting || view.profileLoading || id === view.state?.running) {
      return;
    }
    const generation = ++profileGeneration;
    update({ profileLoading: true, profileError: null });
    try {
      const profile = await transport.invoke<Section<Profile>>(COMMANDS.getRunProfile, { id });
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

  function selectRun(id: string | null) {
    ++detailGeneration;
    ++profileGeneration;
    update({
      consoleOpen: false,
      selectedRun: id,
      detail: null,
      detailError: null,
      detailLoading: false,
      profile: null,
      profileLoading: false,
      profileError: null,
    });
    loadSelected();
  }

  // Only lifecycle transitions call this; reopening a completed Console stays put.
  function selectCompletedConsole(id: string | null | undefined): boolean {
    if (
      !id ||
      !view.consoleOpen ||
      view.console?.id !== id ||
      view.starting ||
      view.state?.running
    ) {
      return false;
    }
    selectRun(id);
    return true;
  }

  // Console read access outlives process controls for the last local process.
  function openConsole(record: RunRecord) {
    if (record.id !== (view.state?.consoleRun ?? view.state?.running)) {
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

  function resetWorkspaceData() {
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
      snapshot: null,
      snapshotLoading: false,
      snapshotError: null,
      selectedTarget: null,
      targetDetail: null,
      targetDetailError: null,
    });
  }

  function resetTargetData() {
    update({
      snapshot: null,
      snapshotLoading: false,
      snapshotError: null,
      selectedTarget: null,
      targetDetail: null,
      targetDetailError: null,
    });
  }

  function acceptState(state: AppState) {
    if (disposed) return;
    const before = view.state;
    const resumed = !view.ready;
    const workspaceChanged = state.workspace !== before?.workspace;
    const scopeChanged =
      JSON.stringify(state.scope.include) !== JSON.stringify(before?.scope.include);
    const contextChanged = workspaceChanged || scopeChanged;
    ++stateRevision;
    if (workspaceChanged) {
      ++listGeneration;
      ++detailGeneration;
      ++profileGeneration;
    }
    if (contextChanged) {
      ++snapshotGeneration;
      ++targetGeneration;
      snapshotRequested = null;
    }
    if (workspaceChanged) {
      resetWorkspaceData();
    } else if (scopeChanged) {
      resetTargetData();
    }
    update({
      state,
      ready: true,
      ...(resumed ? { error: null } : {}),
    });
    openPendingConsole(state);
    syncTargetData(state, before, contextChanged);
    if (resumed || workspaceChanged || state.running !== before?.running) {
      void loadRuns();
    }
    syncCompletedRun(state, before, resumed);
  }

  // A started run claims the Console only when its own workspace grants it.
  function openPendingConsole(state: AppState) {
    if (
      !pendingStart ||
      pendingStart.workspace !== state.workspace ||
      !state.running ||
      state.consoleRun !== state.running
    ) {
      return;
    }
    openConsole({ id: state.running, cmd: pendingStart.cmd, target: pendingStart.target });
  }

  function syncCompletedRun(state: AppState, before: AppState | null, resumed: boolean) {
    if (!resumed && (!before?.running || before.running === state.running)) return;
    const completed = before?.running ?? (resumed ? state.consoleRun : undefined);
    if (selectCompletedConsole(completed)) return;
    if (resumed || view.selectedRun === completed) loadSelected();
  }

  function syncTargetData(state: AppState, before: AppState | null, contextChanged: boolean) {
    if (!contextChanged && state.snapshotFetchedAt !== before?.snapshotFetchedAt) {
      ++targetGeneration;
      update({
        targetDetail: null,
        targetDetailError: null,
      });
    }
    const wantFetchedAt = state.snapshotFetchedAt;
    if (wantFetchedAt === null) {
      clearSnapshot();
      if (view.selectedTarget) selectTarget(null);
    } else if (
      // A fetch for this snapshot may already be in flight; re-issuing would
      // duplicate the transfer this split exists to avoid.
      wantFetchedAt !== view.snapshot?.meta.fetchedAt &&
      wantFetchedAt !== snapshotRequested
    ) {
      void loadSnapshot();
    }
    requestTargetDetail();
  }

  // Defensive: the core drops its snapshot only on a workspace switch,
  // which the reset above already clears.
  function clearSnapshot() {
    if (view.snapshot || view.snapshotError || snapshotRequested) {
      snapshotRequested = null;
      ++snapshotGeneration;
      update({ snapshot: null, snapshotLoading: false, snapshotError: null });
    }
  }

  function dispose() {
    disposed = true;
    unlisten?.();
    unlisten = undefined;
    unlistenHistory?.();
    unlistenHistory = undefined;
  }

  async function init() {
    if (!(await subscribe())) return;
    try {
      const revision = stateRevision;
      const state = await transport.invoke<AppState>(COMMANDS.getState);
      if (revision === stateRevision) acceptState(state);
    } catch (error) {
      // A state event can recover a failed read, or already have superseded it.
      if (!view.ready) update({ error: `Could not connect: ${String(error)}` });
    }
  }

  // Returns false when disposed mid-registration or a subscription failed.
  async function subscribe() {
    try {
      const removeHistory = await transport.listen<void>("history-changed", () => {
        void loadRuns();
      });
      if (disposed) {
        removeHistory();
        return false;
      }
      unlistenHistory = removeHistory;
      const remove = await transport.listen<AppState>("app-state", acceptState);
      if (disposed) {
        remove();
        return false;
      }
      unlisten = remove;
      return true;
    } catch (error) {
      unlistenHistory?.();
      unlistenHistory = undefined;
      if (!disposed) update({ error: `Could not connect: ${String(error)}` });
      return false;
    }
  }

  async function reconcileStartedState(workspace: string) {
    // Invoke responses and events use different Tauri delivery paths. The
    // reply may precede its running event, so reconcile before deciding to
    // read completed files. Events received during this read take priority.
    const revision = stateRevision;
    try {
      const state = await transport.invoke<AppState>(COMMANDS.getState);
      if (disposed || workspace !== view.state?.workspace) return;
      if (revision === stateRevision) acceptState(state);
    } catch (error) {
      if (workspace === view.state?.workspace && revision === stateRevision) {
        update({
          ready: false,
          error: `The run started, but getting its state failed; waiting for the next update: ${String(error)}`,
        });
      }
    }
  }

  async function start(cmd: RunCmd, target: string, argumentsText = "") {
    if (!canSend(view, target)) return;
    const workspace = view.state?.workspace;
    if (!workspace) return;
    let startedRun: string | undefined;
    pendingStart = { workspace, cmd, target };
    update({ starting: true, error: null });
    try {
      // The core parses arguments once; the target stays a separate token.
      const record = await transport.invoke<RunRecord>(COMMANDS.startRun, {
        cmd,
        target,
        arguments: argumentsText,
      });
      pendingStart = null;
      if (disposed || workspace !== view.state?.workspace) return;
      startedRun = record.id;
      openStartedConsole(record, cmd, target);
      await reconcileStartedState(workspace);
      if (disposed || workspace !== view.state?.workspace) return;
      // The run may already have finished before start_run's reply. Never
      // write that reply back into the authoritative running state.
      void loadRuns();
    } catch (error) {
      if (workspace === view.state?.workspace) update({ error: String(error) });
    } finally {
      finishStart(workspace, startedRun);
    }
  }

  function openStartedConsole(record: RunRecord, cmd: RunCmd, target: string) {
    if (view.consoleOpen && view.console?.id === record.id) return;
    selectRun(null);
    // Composer input labels the live Console until build events report it.
    update({ console: { ...record, cmd, target }, consoleOpen: true });
  }

  function finishStart(workspace: string, startedRun: string | undefined) {
    pendingStart = null;
    update({ starting: false });
    requestTargetDetail();
    if (disposed || workspace !== view.state?.workspace || !view.ready) return;
    if (!selectCompletedConsole(startedRun)) loadSelected();
  }

  async function stop() {
    if (!canStop(view)) return;
    const workspace = view.state?.workspace;
    try {
      await transport.invoke(COMMANDS.stopRun);
    } catch (error) {
      if (workspace === view.state?.workspace) update({ error: String(error) });
    }
  }

  async function openWorkspace(path: string, scope: TargetScope) {
    if (
      !view.ready ||
      view.opening ||
      view.starting ||
      view.state?.running ||
      view.state?.ingest === "running" ||
      view.state?.query === "running" ||
      view.state?.detailQuery === "running"
    ) {
      throw new Error(
        "Wait for the current Bazel operation to finish before changing the workspace",
      );
    }
    update({ opening: true });
    const revision = stateRevision;
    try {
      const state = await transport.invoke<AppState>(COMMANDS.openWorkspace, { path, scope });
      if (revision === stateRevision) acceptState(state);
    } finally {
      update({ opening: false });
      requestTargetDetail();
    }
  }

  async function refresh() {
    if (
      !view.ready ||
      view.starting ||
      view.opening ||
      view.state?.running ||
      view.state?.ingest === "running" ||
      view.state?.query === "running" ||
      view.state?.detailQuery === "running"
    ) {
      return;
    }
    try {
      await transport.invoke(COMMANDS.refresh);
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
    loadSnapshot,
    selectTarget,
  };
}
