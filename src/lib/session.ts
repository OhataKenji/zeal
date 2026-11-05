import { COMMANDS } from "./commands.ts";
import { targetError } from "./target.ts";
import type {
  AppState,
  ExplorerSelection,
  BazelCommand,
  InvocationDetail,
  InvocationRecord,
  Profile,
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

/** UI state for one workspace and its selected invocation. */
export interface SessionView {
  console: InvocationRecord | null;
  consoleOpen: boolean;
  state: AppState | null;
  /** Tree and search data for `state.snapshotFetchedAt`. */
  snapshot: TargetIndex | null;
  snapshotLoading: boolean;
  snapshotError: string | null;
  selection: ExplorerSelection;
  targetDetail: TargetDetail | null;
  targetDetailError: string | null;
  ready: boolean;
  starting: boolean;
  opening: boolean;
  error: string | null;
  invocations: InvocationRecord[];
  invocationsError: string | null;
  selectedInvocation: string | null;
  detail: InvocationDetail | null;
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
    selection: null,
    targetDetail: null,
    targetDetailError: null,
    ready: false,
    starting: false,
    opening: false,
    error: null,
    invocations: [],
    invocationsError: null,
    selectedInvocation: null,
    detail: null,
    detailLoading: false,
    detailError: null,
    profile: null,
    profileLoading: false,
    profileError: null,
  };
}

/** Serialized Bazel slots; the core emits each as a binary "idle" | "running" flag. */
const RUNNING_SLOTS = ["ingest", "query", "detailQuery"] as const;

/** Whether a serialized Bazel operation occupies the workspace. Matches the core's bazel_busy. */
function bazelBusy(state: AppState | null): boolean {
  return bazelBusyReason(state) !== null;
}

export function bazelBusyReason(state: AppState | null): string | null {
  if (state?.activeInvocation) return "a Bazel invocation is in progress";
  const reasons = {
    ingest: "targets are loading",
    query: "a query is already running",
    detailQuery: "target details are loading",
  };
  const slot = RUNNING_SLOTS.find((slot) => state?.[slot] === "running");
  return slot ? reasons[slot] : null;
}

/** Whether the session is not ready or a lifecycle request is in flight. */
function lifecycleBusy(view: SessionView): boolean {
  return !view.ready || view.opening || view.starting;
}

/** Whether a lifecycle transition or a serialized Bazel operation is in flight. */
export function sessionBusy(view: SessionView): boolean {
  return lifecycleBusy(view) || bazelBusy(view.state);
}

/** Whether the core knows the workspace and its Bazel binary. */
function workspaceReady(view: SessionView): boolean {
  return !!(view.state?.workspace && view.state.bazel);
}

/**
 * Returns whether manual target input can start now. A loaded snapshot is not
 * required, so the composer remains usable after ingestion errors settle.
 */
export function canSend(view: SessionView, target: string): boolean {
  return !!(!sessionBusy(view) && workspaceReady(view) && targetError(target) === null);
}

/**
 * Returns whether the core can stop an active invocation that is not already
 * stopping. Core remains responsible for releasing an unconfirmed invocation
 * with a note.
 */
export function canStop(view: SessionView): boolean {
  return !!(view.ready && view.state?.activeInvocation && !view.state.stopping);
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
  let pendingStart: { workspace: string; cmd: BazelCommand; pattern: string } | null = null;
  let unlisten: (() => void) | undefined;
  let unlistenHistory: (() => void) | undefined;
  function update(patch: Partial<SessionView>) {
    if (disposed) return;
    view = { ...view, ...patch };
    changed(view);
  }

  // Clearing a slice invalidates its in-flight loads, so a late response
  // cannot repopulate cleared data.
  function clearDetail() {
    ++detailGeneration;
    return { detail: null, detailLoading: false, detailError: null };
  }
  function clearProfile() {
    ++profileGeneration;
    return { profile: null, profileLoading: false, profileError: null };
  }
  function clearInvocations() {
    ++listGeneration;
    return { invocations: [], invocationsError: null };
  }
  function clearTargetDetail() {
    ++targetGeneration;
    return { targetDetail: null, targetDetailError: null };
  }
  function clearTargets() {
    ++snapshotGeneration;
    snapshotRequested = null;
    return {
      snapshot: null,
      snapshotLoading: false,
      snapshotError: null,
      selection: null,
      ...clearTargetDetail(),
    };
  }

  async function loadInvocations() {
    if (!view.state?.workspace || disposed) return;
    if (listing) {
      listPending = true;
      return;
    }
    listing = true;
    const generation = ++listGeneration;
    try {
      const invocations = await transport.invoke<InvocationRecord[]>(COMMANDS.listInvocations);
      acceptLoadedInvocations(generation, invocations);
    } catch (error) {
      if (generation === listGeneration) {
        update({ invocationsError: String(error) });
      }
    } finally {
      listing = false;
      if (listPending && !disposed) {
        listPending = false;
        void loadInvocations();
      }
    }
  }

  // A changed selected record means its saved observation may have completed.
  function acceptLoadedInvocations(generation: number, invocations: InvocationRecord[]) {
    if (generation !== listGeneration || disposed) return;
    if (JSON.stringify(invocations) !== JSON.stringify(view.invocations) || view.invocationsError) {
      update({ invocations, invocationsError: null });
    }
    const selected = invocations.find((inv) => inv.id === view.selectedInvocation);
    if (
      !selected ||
      !view.detail ||
      view.detailLoading ||
      view.detailError ||
      JSON.stringify(selected) === JSON.stringify(view.detail.record)
    ) {
      return;
    }
    loadSelected();
  }

  // The DTO carries only the snapshot's fetchedAt; its data is fetched when
  // that timestamp changes, not on every state event.
  function acceptLoadedSnapshot(snapshot: TargetIndex | null) {
    snapshotRequested = null;
    update({ snapshot, snapshotLoading: false, snapshotError: null, targetDetailError: null });
    const selection = view.selection;
    if (
      selection?.kind === "package" ||
      (selection?.kind === "target" && !snapshot?.targets.some((t) => t.label === selection.label))
    ) {
      select(null);
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
      ...(view.selection?.kind === "target"
        ? { targetDetailError: "Could not load the target list" }
        : {}),
    });
  }

  function targetBusy(): boolean {
    return sessionBusy(view) || !workspaceReady(view);
  }

  function requestTargetDetail() {
    const label = view.selection?.kind === "target" ? view.selection.label : null;
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

  function select(selection: ExplorerSelection) {
    if (
      selection?.kind === "package" &&
      view.selection?.kind === "package" &&
      selection.path === view.selection.path
    )
      return;
    if (
      selection?.kind === "target" &&
      view.selection?.kind === "target" &&
      selection.label === view.selection.label &&
      !view.targetDetailError
    )
      return;
    update({ selection, ...clearTargetDetail() });
    requestTargetDetail();
  }

  async function loadDetail() {
    const id = view.selectedInvocation;
    // Saved observations are read after the active invocation ends.
    if (
      !id ||
      !view.ready ||
      view.starting ||
      view.detailLoading ||
      id === view.state?.activeInvocation
    ) {
      return;
    }
    const generation = ++detailGeneration;
    update({ detailLoading: true, detailError: null });
    try {
      const detail = await transport.invoke<InvocationDetail>(COMMANDS.getInvocation, { id });
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
    const id = view.selectedInvocation;
    if (
      !id ||
      !view.ready ||
      view.starting ||
      view.profileLoading ||
      id === view.state?.activeInvocation
    ) {
      return;
    }
    const generation = ++profileGeneration;
    update({ profileLoading: true, profileError: null });
    try {
      const profile = await transport.invoke<Section<Profile>>(COMMANDS.getInvocationProfile, {
        id,
      });
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

  function selectInvocation(id: string | null) {
    update({
      consoleOpen: false,
      selectedInvocation: id,
      ...clearDetail(),
      ...clearProfile(),
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
      view.state?.activeInvocation
    ) {
      return false;
    }
    selectInvocation(id);
    return true;
  }

  // Console read access outlives process controls for the last local process.
  function openConsole(record: InvocationRecord) {
    if (record.id !== (view.state?.consoleInvocation ?? view.state?.activeInvocation)) {
      selectInvocation(record.id);
      return;
    }
    if (view.consoleOpen && view.console?.id === record.id) return;
    update({
      console: record,
      consoleOpen: true,
      selectedInvocation: null,
      ...clearDetail(),
      ...clearProfile(),
    });
  }

  function resetWorkspaceData() {
    update({
      console: null,
      consoleOpen: false,
      selectedInvocation: null,
      ...clearInvocations(),
      ...clearDetail(),
      ...clearProfile(),
      ...clearTargets(),
    });
  }

  function resetTargetData() {
    update(clearTargets());
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
    if (resumed || workspaceChanged || state.activeInvocation !== before?.activeInvocation) {
      void loadInvocations();
    }
    syncCompletedInvocation(state, before, resumed);
  }

  // A started invocation claims the Console only when its own workspace grants it.
  function openPendingConsole(state: AppState) {
    if (
      !pendingStart ||
      pendingStart.workspace !== state.workspace ||
      !state.activeInvocation ||
      state.consoleInvocation !== state.activeInvocation
    ) {
      return;
    }
    openConsole({
      id: state.activeInvocation,
      cmd: pendingStart.cmd,
      pattern: pendingStart.pattern,
    });
  }

  function syncCompletedInvocation(state: AppState, before: AppState | null, resumed: boolean) {
    if (
      !resumed &&
      (!before?.activeInvocation || before.activeInvocation === state.activeInvocation)
    )
      return;
    const completed = before?.activeInvocation ?? (resumed ? state.consoleInvocation : undefined);
    if (selectCompletedConsole(completed)) return;
    if (resumed || view.selectedInvocation === completed) loadSelected();
  }

  function syncTargetData(state: AppState, before: AppState | null, contextChanged: boolean) {
    if (!contextChanged && state.snapshotFetchedAt !== before?.snapshotFetchedAt) {
      update(clearTargetDetail());
    }
    const wantFetchedAt = state.snapshotFetchedAt;
    if (wantFetchedAt === null) {
      clearSnapshot();
      if (view.selection) select(null);
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
      const removeHistory = await transport.listen<void>("invocations-changed", () => {
        void loadInvocations();
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
          error: `The invocation started, but getting its state failed; waiting for the next update: ${String(error)}`,
        });
      }
    }
  }

  async function start(cmd: BazelCommand, pattern: string, argumentsText = "") {
    if (!canSend(view, pattern)) return;
    const workspace = view.state?.workspace;
    if (!workspace) return;
    let startedInvocation: string | undefined;
    pendingStart = { workspace, cmd, pattern };
    update({ starting: true, error: null });
    try {
      // The core parses arguments once; the pattern stays a separate token.
      const record = await transport.invoke<InvocationRecord>(COMMANDS.startInvocation, {
        cmd,
        pattern,
        arguments: argumentsText,
      });
      pendingStart = null;
      if (disposed || workspace !== view.state?.workspace) return;
      startedInvocation = record.id;
      openStartedConsole(record, cmd, pattern);
      await reconcileStartedState(workspace);
      if (disposed || workspace !== view.state?.workspace) return;
      // The invocation may already have finished before start_invocation's
      // reply. Never write that reply back into the authoritative active state.
      void loadInvocations();
    } catch (error) {
      if (workspace === view.state?.workspace) update({ error: String(error) });
    } finally {
      finishStart(workspace, startedInvocation);
    }
  }

  function openStartedConsole(record: InvocationRecord, cmd: BazelCommand, pattern: string) {
    if (view.consoleOpen && view.console?.id === record.id) return;
    selectInvocation(null);
    // Composer input labels the live Console until build events report it.
    update({ console: { ...record, cmd, pattern }, consoleOpen: true });
  }

  function finishStart(workspace: string, startedInvocation: string | undefined) {
    pendingStart = null;
    update({ starting: false });
    requestTargetDetail();
    if (disposed || workspace !== view.state?.workspace || !view.ready) return;
    if (!selectCompletedConsole(startedInvocation)) loadSelected();
  }

  async function stop() {
    if (!canStop(view)) return;
    const workspace = view.state?.workspace;
    try {
      await transport.invoke(COMMANDS.stopInvocation);
    } catch (error) {
      if (workspace === view.state?.workspace) update({ error: String(error) });
    }
  }

  async function openWorkspace(path: string, scope: TargetScope) {
    if (sessionBusy(view)) {
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
    if (sessionBusy(view)) {
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
    selectInvocation,
    openConsole,
    loadDetail,
    loadProfile,
    loadInvocations,
    loadSnapshot,
    select,
  };
}
