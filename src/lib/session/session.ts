import { COMMANDS, type Transport } from "../commands.ts";
import { targetError } from "../target.ts";
import { createTargets, emptyTargets, type TargetsView } from "./targets.ts";
import { createInvocations, emptyInvocations, type InvocationsView } from "./invocations.ts";
import type {
  AppState,
  ExplorerSelection,
  BazelCommand,
  InvocationRecord,
  TargetScope,
} from "../types.ts";

export type Page =
  | { kind: "explorer" }
  | { kind: "query" }
  | { kind: "analytics" }
  | { kind: "invocation"; id: string }
  | { kind: "console" };
export type MainPage = "explorer" | "query" | "analytics";
export function invocationId(page: Page): string | null {
  return page.kind === "invocation" ? page.id : null;
}

/** UI state for one workspace and its selected invocation. */
export interface SessionView {
  console: InvocationRecord | null;
  page: Page;
  state: AppState | null;
  ready: boolean;
  starting: boolean;
  opening: boolean;
  error: string | null;
  targets: TargetsView;
  invocations: InvocationsView;
}

/** Creates the empty state shown before a workspace has loaded. */
export function emptyView(initialPage: MainPage = "explorer"): SessionView {
  return {
    console: null,
    page: { kind: initialPage },
    state: null,
    ready: false,
    starting: false,
    opening: false,
    error: null,
    targets: emptyTargets(),
    invocations: emptyInvocations(),
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
export function createSession(
  transport: Transport,
  changed: (view: SessionView) => void,
  initialPage: MainPage = "explorer",
) {
  let view = emptyView(initialPage);
  let disposed = false;
  let stateRevision = 0;
  let pendingStart: { workspace: string; cmd: BazelCommand; pattern: string } | null = null;
  let unlisten: (() => void) | undefined;
  let unlistenHistory: (() => void) | undefined;
  let batchDepth = 0;
  let dirty = false;
  function update(patch: Partial<SessionView>) {
    if (disposed) return;
    view = { ...view, ...patch };
    if (batchDepth) dirty = true;
    else changed(view);
  }
  function batch(action: () => void) {
    batchDepth++;
    try {
      action();
    } finally {
      batchDepth--;
      if (!batchDepth && dirty) {
        dirty = false;
        if (!disposed) changed(view);
      }
    }
  }
  const targets = createTargets(transport, (targets) => update({ targets }));
  const invocations = createInvocations(transport, (invocations) => update({ invocations }));
  function syncReads() {
    if (disposed) return;
    targets.sync({
      workspace: view.state?.workspace ?? null,
      scopeKey: JSON.stringify(view.state?.scope.include ?? []),
      snapshotFetchedAt: view.state?.snapshotFetchedAt ?? null,
      canReadDetail: !sessionBusy(view) && workspaceReady(view),
    });
    invocations.sync({
      workspace: view.state?.workspace ?? null,
      selectedId: invocationId(view.page),
      activeId: view.state?.activeInvocation ?? null,
      canReadArtifacts: view.ready && !view.starting,
    });
  }

  function showPage(kind: MainPage) {
    if (view.page.kind === kind) return;
    batch(() => {
      update({ page: { kind } });
      syncReads();
    });
  }

  function selectInvocation(id: string) {
    const same = invocationId(view.page) === id;
    batch(() => {
      update({ page: { kind: "invocation", id } });
      syncReads();
      if (same) invocations.clearSelected();
    });
    invocations.loadSelected();
  }

  // Only lifecycle transitions call this; reopening a completed Console stays put.
  function selectCompletedConsole(id: string | null | undefined): boolean {
    if (
      !id ||
      view.page.kind !== "console" ||
      view.console?.id !== id ||
      view.starting ||
      view.state?.activeInvocation
    ) {
      return false;
    }
    selectInvocation(id);
    return true;
  }

  function select(selection: ExplorerSelection) {
    batch(() => {
      showPage("explorer");
      targets.select(selection);
      syncReads();
    });
  }

  // Console read access outlives process controls for the last local process.
  function openConsole(record: InvocationRecord) {
    if (record.id !== (view.state?.consoleInvocation ?? view.state?.activeInvocation)) {
      selectInvocation(record.id);
      return;
    }
    if (view.page.kind === "console" && view.console?.id === record.id) return;
    batch(() => {
      update({
        console: record,
        page: { kind: "console" },
      });
      syncReads();
    });
  }

  function resetWorkspaceData() {
    const page: Page =
      view.page.kind === "query" || view.page.kind === "analytics"
        ? view.page
        : { kind: "explorer" };
    update({ console: null, page });
  }

  function acceptState(state: AppState) {
    if (disposed) return;
    batch(() => {
      const before = view.state;
      const resumed = !view.ready;
      const workspaceChanged = state.workspace !== before?.workspace;
      ++stateRevision;
      if (workspaceChanged) {
        resetWorkspaceData();
      }
      update({
        state,
        ready: true,
        ...(resumed ? { error: null } : {}),
      });
      syncReads();
      openPendingConsole(state);
      if (resumed || workspaceChanged || state.activeInvocation !== before?.activeInvocation) {
        void invocations.loadList();
      }
      syncCompletedInvocation(state, before, resumed);
    });
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
    if (resumed || invocationId(view.page) === completed) invocations.loadSelected();
  }

  function dispose() {
    disposed = true;
    targets.dispose();
    invocations.dispose();
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
        void invocations.loadList();
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
        batch(() => {
          update({
            ready: false,
            error: `The invocation started, but getting its state failed; waiting for the next update: ${String(error)}`,
          });
          syncReads();
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
    batch(() => {
      update({ starting: true, error: null });
      syncReads();
    });
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
      void invocations.loadList();
    } catch (error) {
      if (workspace === view.state?.workspace) update({ error: String(error) });
    } finally {
      finishStart(workspace, startedInvocation);
    }
  }

  function openStartedConsole(record: InvocationRecord, cmd: BazelCommand, pattern: string) {
    if (view.page.kind === "console" && view.console?.id === record.id) return;
    // Composer input labels the live Console until build events report it.
    batch(() => {
      update({
        console: { ...record, cmd, pattern },
        page: { kind: "console" },
      });
      syncReads();
    });
  }

  function finishStart(workspace: string, startedInvocation: string | undefined) {
    if (disposed) return;
    pendingStart = null;
    batch(() => {
      update({ starting: false });
      syncReads();
      if (workspace !== view.state?.workspace || !view.ready) return;
      if (!selectCompletedConsole(startedInvocation)) invocations.loadSelected();
    });
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
    batch(() => {
      update({ opening: true });
      syncReads();
    });
    const revision = stateRevision;
    try {
      const state = await transport.invoke<AppState>(COMMANDS.openWorkspace, { path, scope });
      if (revision === stateRevision) acceptState(state);
    } finally {
      batch(() => {
        update({ opening: false });
        syncReads();
      });
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
    showPage,
    selectInvocation,
    openConsole,
    select,
    loadDetail: invocations.loadDetail,
    loadProfile: invocations.loadProfile,
    loadInvocations: invocations.loadList,
    loadSnapshot: targets.loadSnapshot,
  };
}
