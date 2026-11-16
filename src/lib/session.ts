import { COMMANDS } from "./commands.ts";
import { targetError } from "./target.ts";
import type {
  AppState,
  BazelCommand,
  ExplorerSelection,
  InvocationDetail,
  InvocationRecord,
  OpenWorkspaceResult,
  OwnedInvocation,
  Profile,
  Section,
  TargetActions,
  TargetDetail,
  TargetIndex,
  TargetLoad,
  TargetOutputs,
  TargetScope,
  WorkspaceState,
} from "./types.ts";

/** Workspace page destinations inside the main content area. */
export type WorkspacePage = "targets" | "analytics" | "query";

/** The single destination the session is showing. */
export type SessionContent =
  | { readonly kind: WorkspacePage }
  | { readonly kind: "invocation"; readonly id: string; readonly page: WorkspacePage };

/** IPC boundary used by a session to call the core and subscribe to events. */
export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, callback: (payload: T) => void): Promise<() => void>;
}

interface Reader<T> {
  value: T | null;
  error: string | null;
  loading: boolean;
}

interface TargetSelection {
  selection: ExplorerSelection;
  detail: Reader<TargetDetail>;
  outputs: Reader<TargetOutputs>;
  actions: Reader<TargetActions>;
}

interface Workspace {
  path: string;
  snapshot: TargetIndex | null;
  snapshotError: string | null;
  refresh: object | null;
  target: TargetSelection;
  targetBusy: boolean;
  history: InvocationRecord[];
  historyError: string | null;
  historyBusy: boolean;
  historyDirty: boolean;
}

interface Saved {
  detail: Reader<InvocationDetail>;
  profile: Reader<Section<Profile>>;
}

interface SessionState {
  core: AppState | null;
  workspace: Workspace | null;
  content: SessionContent;
  navigation: number;
  start: { navigation: number } | null;
  prompt: {
    status: "closed" | "editing" | "opening";
    error: string | null;
    request: object | null;
  };
  error: string | null;
  saved: Saved;
}

function reader<T>(value: T | null = null): Reader<T> {
  return { value, error: null, loading: false };
}

function targetSelection(selection: ExplorerSelection = null): TargetSelection {
  return { selection, detail: reader(), outputs: reader(), actions: reader() };
}

function savedReaders(): Saved {
  return { detail: reader(), profile: reader() };
}

function initialState(page: WorkspacePage): SessionState {
  return {
    core: null,
    workspace: null,
    content: { kind: page },
    navigation: 0,
    start: null,
    prompt: { status: "closed", error: null, request: null },
    error: null,
    saved: savedReaders(),
  };
}

function wsEntry(core: AppState | null, path: string | null): WorkspaceState | null {
  if (!core || path === null) return null;
  return core.workspaces.find((entry) => entry.workspace === path) ?? null;
}

function ownedEntry(core: AppState | null, id: string | null): OwnedInvocation | null {
  if (!core || id === null) return null;
  return core.invocations.find((entry) => entry.id === id) ?? null;
}

function selectedInvocationId(state: SessionState): string | null {
  return state.content.kind === "invocation" ? state.content.id : null;
}

function selectedExecution(state: SessionState): OwnedInvocation | null {
  const entry = ownedEntry(state.core, selectedInvocationId(state));
  return entry && entry.workspace === state.workspace?.path ? entry : null;
}

function workspacePage(state: SessionState): WorkspacePage {
  const content = state.content;
  return content.kind === "invocation" ? content.page : content.kind;
}

function savedEligible(state: SessionState): boolean {
  const id = selectedInvocationId(state);
  return !!(
    id &&
    state.core !== null &&
    state.workspace !== null &&
    !selectedExecution(state)?.console &&
    state.workspace.history.some((record) => record.id === id)
  );
}

function sameScope(left: WorkspaceState, right: WorkspaceState): boolean {
  return JSON.stringify(left.scope.include) === JSON.stringify(right.scope.include);
}

function sameSelection(left: ExplorerSelection, right: ExplorerSelection): boolean {
  if (left?.kind !== right?.kind) return false;
  if (left?.kind === "target" && right?.kind === "target") return left.label === right.label;
  if (left?.kind === "package" && right?.kind === "package") return left.path === right.path;
  return left === null && right === null;
}

function sameContent(left: SessionContent, right: SessionContent): boolean {
  if (left.kind !== right.kind) return false;
  if (left.kind === "invocation" && right.kind === "invocation") {
    return left.id === right.id && left.page === right.page;
  }
  return true;
}

/** Flat projection of session state consumed by the Svelte views. */
export type SessionView = ReturnType<typeof sessionView>;

function sessionView(state: SessionState) {
  const eligible = savedEligible(state);
  const core = state.core;
  const ws = state.workspace;
  // Every owned entry is a live worker: all are pinned, and a released
  // invocation simply leaves the map so its history row stands alone.
  const owned = (core?.invocations ?? [])
    .filter((entry) => entry.workspace === ws?.path)
    .sort((a, b) => b.startedAt - a.startedAt);
  const pinnedIds = new Set(owned.map((entry) => entry.id));
  const historyById = new Map((ws?.history ?? []).map((record) => [record.id, record]));
  const invocationRows: Array<{ record: InvocationRecord; execution?: OwnedInvocation }> = [
    ...owned.map((execution) => ({
      record: historyById.get(execution.id) ?? execution,
      execution,
    })),
    ...(ws?.history ?? [])
      .filter((record) => !pinnedIds.has(record.id))
      .map((record) => ({ record })),
  ];
  const execution = selectedExecution(state);
  const consoleRecord: InvocationRecord | null =
    execution?.console === true ? (historyById.get(execution.id) ?? execution) : null;
  const target = ws?.target ?? null;
  const saved = state.saved;
  return {
    content: state.content,
    console: consoleRecord,
    core,
    state: wsEntry(core, ws?.path ?? null),
    invocationRows,
    execution,
    snapshot: ws?.snapshot ?? null,
    snapshotLoading: ws !== null && ws.refresh !== null,
    snapshotError: ws?.snapshotError ?? null,
    selection: target?.selection ?? null,
    targetDetail: target?.detail.value ?? null,
    targetDetailError: target?.detail.error ?? null,
    targetOutputs: target?.outputs.value ?? null,
    targetOutputsError: target?.outputs.error ?? null,
    targetActions: target?.actions.value ?? null,
    targetActionsError: target?.actions.error ?? null,
    ready: core !== null,
    starting: state.start !== null,
    opening: state.prompt.status === "opening",
    workspacePromptOpen: state.prompt.status !== "closed",
    workspaceError: state.prompt.error,
    error: state.error,
    invocations: ws?.history ?? [],
    invocationsError: ws?.historyError ?? null,
    detail: saved.detail.value,
    detailError: saved.detail.error,
    profile: saved.profile.value,
    detailLoading: eligible && saved.detail.loading,
    profileLoading: eligible && saved.profile.loading,
    profileError: saved.profile.error,
  };
}

/** Whether the composer may send: connected, scoped workspace, Bazel and BES up. */
export function canSend(view: SessionView, target: string): boolean {
  return !!(
    view.ready &&
    view.state &&
    view.core?.bazel &&
    view.core.besReady &&
    !view.starting &&
    targetError(target) === null
  );
}

export function emptyView(page: WorkspacePage = "targets"): SessionView {
  return sessionView(initialState(page));
}

/**
 * Owns the session's mutable state and drives every IPC read directly.
 * Stale async results are rejected by captured object identity: workspace,
 * target selection, saved readers, refresh token, start token, and prompt
 * request each invalidate only their own pending work.
 */
export function createSession(
  transport: Transport,
  changed: (view: SessionView) => void,
  page: WorkspacePage = "targets",
) {
  const state = initialState(page);
  let disposed = false;
  let subscribed = false;
  let initializing: Promise<void> | undefined;
  const unlistens: Array<() => void> = [];

  function publish() {
    changed(sessionView(state));
  }

  /** Runs one read into a reader slot unless it is busy or already stale. */
  async function read<T>(
    slot: Reader<T>,
    operation: () => Promise<T>,
    current: () => boolean,
  ): Promise<boolean> {
    if (disposed || slot.loading || !current()) return false;
    slot.loading = true;
    slot.error = null;
    publish();
    try {
      const value = await operation();
      if (!current()) return false;
      slot.value = value;
    } catch (error) {
      if (!current()) return false;
      slot.error = String(error);
    } finally {
      slot.loading = false;
      if (current()) publish();
    }
    return current() && slot.error === null;
  }

  /** The selected target's detail -> outputs -> actions chain, one at a time. */
  async function loadTarget() {
    const ws = state.workspace;
    const target = ws?.target;
    const selection = target?.selection;
    if (
      !ws ||
      !target ||
      ws.targetBusy ||
      ws.refresh !== null ||
      !state.core?.bazel ||
      selection?.kind !== "target" ||
      !wsEntry(state.core, ws.path) ||
      !ws.snapshot?.targets.some((item) => item.label === selection.label)
    )
      return;
    const args = { workspace: ws.path, label: selection.label };
    const current = () =>
      !disposed && state.workspace === ws && ws.target === target && ws.refresh === null;
    ws.targetBusy = true;
    try {
      const loaded = await read(
        target.detail,
        async () => {
          const value = await transport.invoke<TargetDetail>(COMMANDS.getTargetDetail, args);
          if (value.target.label !== args.label) {
            throw new Error("Target detail does not match the selected label");
          }
          return value;
        },
        current,
      );
      if (!loaded) return;
      await read(
        target.outputs,
        () => transport.invoke<TargetOutputs>(COMMANDS.getTargetOutputs, args),
        current,
      );
      if (!current()) return;
      await read(
        target.actions,
        () => transport.invoke<TargetActions>(COMMANDS.getTargetActions, args),
        current,
      );
    } finally {
      ws.targetBusy = false;
      if (!disposed && state.workspace === ws && ws.target !== target) void loadTarget();
    }
  }

  function acceptSnapshot(ws: Workspace, snapshot: TargetIndex | null) {
    const selection = ws.target.selection;
    const validTarget =
      selection?.kind === "target" &&
      (snapshot?.targets.some((target) => target.label === selection.label) ?? false);
    const selectionInvalid =
      selection?.kind === "package" || (selection?.kind === "target" && !validTarget);
    ws.snapshot = snapshot;
    ws.snapshotError = null;
    ws.target = targetSelection(selectionInvalid ? null : selection);
  }

  async function refreshSnapshot() {
    const ws = state.workspace;
    if (disposed || !ws) return;
    const entry = wsEntry(state.core, ws.path);
    if (!entry || !state.core?.bazel) return;
    const scope = entry.scope;
    const token = {};
    ws.refresh = token;
    ws.snapshotError = null;
    // The old target context is dropped up front; the snapshot decides what
    // selection survives once it lands.
    ws.target = targetSelection(ws.target.selection);
    publish();
    const current = () => !disposed && state.workspace === ws && ws.refresh === token;
    try {
      const load = await transport.invoke<TargetLoad>(COMMANDS.refresh, {
        workspace: ws.path,
        scope,
      });
      if (!current()) return;
      ws.refresh = null;
      if (load.snapshot !== null) acceptSnapshot(ws, load.snapshot);
      ws.snapshotError = load.error?.summary ?? null;
    } catch (error) {
      if (!current()) return;
      ws.refresh = null;
      ws.snapshotError = `Could not load the workspace snapshot: ${String(error)}`;
    }
    publish();
    void loadTarget();
  }

  function updateTargetsForWorkspace(
    before: WorkspaceState | null,
    after: WorkspaceState | null,
    bazelArrived: boolean,
  ) {
    const ws = state.workspace;
    if (!ws) return;
    if (before !== null && after !== null && sameScope(before, after)) {
      if (bazelArrived && ws.snapshot === null && ws.refresh === null) void refreshSnapshot();
      return;
    }
    // A different workspace entry or scope discards the target context; the
    // next refresh belongs to the new scope.
    ws.refresh = null;
    acceptSnapshot(ws, null);
    if (after !== null) void refreshSnapshot();
  }

  function acceptHistory(ws: Workspace, records: InvocationRecord[]) {
    const wasEligible = savedEligible(state);
    const id = selectedInvocationId(state);
    const previous = ws.history.find((record) => record.id === id);
    const next = records.find((record) => record.id === id);
    const changed = JSON.stringify(previous ?? null) !== JSON.stringify(next ?? null);
    const detail = state.saved.detail;
    const detailMismatch =
      next !== undefined &&
      detail.value !== null &&
      !detail.loading &&
      detail.error === null &&
      JSON.stringify(next) !== JSON.stringify(detail.value.record);
    ws.history = records;
    ws.historyError = null;
    if (changed || detailMismatch) refreshSavedFromHistory();
    reconcileSaved(wasEligible);
  }

  function requestHistory() {
    const ws = state.workspace;
    if (disposed || !ws) return;
    if (ws.historyBusy) {
      ws.historyDirty = true;
      return;
    }
    void readHistory(ws);
  }

  async function readHistory(ws: Workspace) {
    ws.historyBusy = true;
    ws.historyError = null;
    publish();
    const current = () => !disposed && state.workspace === ws;
    try {
      const records = await transport.invoke<InvocationRecord[]>(COMMANDS.listInvocations, {
        workspace: ws.path,
      });
      if (current()) acceptHistory(ws, records);
    } catch (error) {
      if (current()) ws.historyError = String(error);
    } finally {
      ws.historyBusy = false;
      if (current()) {
        publish();
        if (ws.historyDirty) {
          ws.historyDirty = false;
          requestHistory();
        }
      }
    }
  }

  function savedContext() {
    const ws = state.workspace;
    const saved = state.saved;
    const id = selectedInvocationId(state);
    if (disposed || !ws || !id || !savedEligible(state)) return null;
    const current = () =>
      !disposed &&
      state.workspace === ws &&
      state.saved === saved &&
      selectedInvocationId(state) === id &&
      savedEligible(state);
    return { ws, saved, id, current };
  }

  function readSavedDetail() {
    const ctx = savedContext();
    if (!ctx) return;
    void read(
      ctx.saved.detail,
      () =>
        transport.invoke<InvocationDetail>(COMMANDS.getInvocation, {
          workspace: ctx.ws.path,
          id: ctx.id,
        }),
      ctx.current,
    );
  }

  function readSavedProfile() {
    const ctx = savedContext();
    if (!ctx) return;
    void read(
      ctx.saved.profile,
      () =>
        transport.invoke<Section<Profile>>(COMMANDS.getInvocationProfile, {
          workspace: ctx.ws.path,
          id: ctx.id,
        }),
      ctx.current,
    );
  }

  /** Starts each eligible saved reader that has never produced a value. */
  function ensureSaved() {
    const ctx = savedContext();
    if (!ctx) return;
    if (
      ctx.saved.detail.value === null &&
      ctx.saved.detail.error === null &&
      !ctx.saved.detail.loading
    ) {
      readSavedDetail();
    }
    if (
      ctx.saved.profile.value === null &&
      ctx.saved.profile.error === null &&
      !ctx.saved.profile.loading
    ) {
      readSavedProfile();
    }
  }

  /** A history update re-reads saved results even when values are cached. */
  function refreshSavedFromHistory() {
    if (selectedInvocationId(state) === null) return;
    const saved = state.saved;
    // Replacing the wrapper orphans any in-flight read; the retained values
    // stay visible until the fresh pair lands or fails.
    state.saved = {
      detail: reader(saved.detail.value),
      profile: reader(saved.profile.value),
    };
    readSavedDetail();
    readSavedProfile();
  }

  /** Eligibility flips replace the readers; a gain starts fresh reads. */
  function reconcileSaved(wasEligible: boolean) {
    const isEligible = savedEligible(state);
    if (wasEligible === isEligible) return;
    if (isEligible) {
      ensureSaved();
      return;
    }
    state.saved = savedReaders();
  }

  function selectWorkspace(path: string) {
    if (disposed || state.workspace?.path === path) return;
    const page = workspacePage(state);
    state.workspace = {
      path,
      snapshot: null,
      snapshotError: null,
      refresh: null,
      target: targetSelection(),
      targetBusy: false,
      history: [],
      historyError: null,
      historyBusy: false,
      historyDirty: false,
    };
    state.navigation++;
    state.start = null;
    state.error = null;
    state.saved = savedReaders();
    state.content = { kind: page };
    publish();
    requestHistory();
    void refreshSnapshot();
  }

  function showContent(content: SessionContent) {
    if (disposed || sameContent(state.content, content)) return;
    state.navigation++;
    const previousId = selectedInvocationId(state);
    state.content = content;
    if (previousId !== selectedInvocationId(state)) {
      state.saved = savedReaders();
    }
    publish();
    ensureSaved();
  }

  function selectInvocation(id: string) {
    if (disposed) return;
    showContent({ kind: "invocation", id, page: workspacePage(state) });
    const saved = state.saved;
    if (!savedEligible(state)) return;
    // Re-selecting the same invocation only retries failed readers.
    if (saved.detail.error !== null) readSavedDetail();
    if (saved.profile.error !== null) readSavedProfile();
  }

  function showPage(page: WorkspacePage) {
    showContent({ kind: page });
  }

  function select(selection: ExplorerSelection) {
    if (disposed) return;
    showContent({ kind: "targets" });
    const ws = state.workspace;
    if (!ws) return;
    const target = ws.target;
    if (sameSelection(target.selection, selection)) {
      if (selection?.kind === "target" && target.detail.error !== null) {
        ws.target = targetSelection(selection);
        publish();
        void loadTarget();
      }
      return;
    }
    state.navigation++;
    ws.target = targetSelection(selection);
    publish();
    void loadTarget();
  }

  function restorableWorkspace(snapshot: AppState): string | null {
    if (
      state.workspace !== null ||
      snapshot.lastWorkspace === null ||
      !snapshot.workspaces.some((entry) => entry.workspace === snapshot.lastWorkspace)
    )
      return null;
    return snapshot.lastWorkspace;
  }

  function applyState(snapshot: AppState) {
    if (disposed) return;
    const before = state.core;
    if (before !== null && snapshot.revision <= before.revision) return;
    const wasEligible = savedEligible(state);
    const wsBefore = state.workspace;
    state.core = snapshot;
    if (before === null) state.error = null;

    const restore = restorableWorkspace(snapshot);
    if (restore !== null) selectWorkspace(restore);
    // A workspace selected above already reset its readers; only an unchanged
    // selection needs its target context reconciled against the new snapshot.
    if (state.workspace !== null && state.workspace === wsBefore) {
      updateTargetsForWorkspace(
        wsEntry(before, state.workspace.path),
        wsEntry(snapshot, state.workspace.path),
        !before?.bazel && snapshot.bazel !== null,
      );
    }
    reconcileSaved(wasEligible);
    publish();
  }

  async function start(cmd: BazelCommand, pattern: string, argumentsText: string) {
    const ws = state.workspace;
    if (disposed || !ws || !canSend(sessionView(state), pattern)) return;
    const pending = { navigation: state.navigation };
    state.start = pending;
    state.error = null;
    publish();
    try {
      const id = await transport.invoke<string>(COMMANDS.startInvocation, {
        workspace: ws.path,
        cmd,
        pattern,
        arguments: argumentsText,
      });
      if (disposed || state.start !== pending) return;
      state.start = null;
      if (pending.navigation !== state.navigation || state.workspace !== ws) {
        publish();
        return;
      }
      showContent({ kind: "invocation", id, page: workspacePage(state) });
    } catch (error) {
      if (disposed || state.start !== pending) return;
      state.start = null;
      if (pending.navigation !== state.navigation || state.workspace !== ws) {
        publish();
        return;
      }
      state.error = String(error);
      publish();
    }
  }

  async function stop(id: string) {
    if (disposed) return;
    const execution = ownedEntry(state.core, id);
    if (!execution?.stoppable || execution.stopping) return;
    try {
      await transport.invoke<void>(COMMANDS.stopInvocation, {
        workspace: execution.workspace,
        id: execution.id,
      });
    } catch (error) {
      if (disposed) return;
      if (state.workspace?.path === execution.workspace && selectedInvocationId(state) === id) {
        state.error = String(error);
        publish();
      }
    }
  }

  async function openWorkspace(path: string, scope: TargetScope) {
    if (disposed) return;
    state.navigation++;
    state.prompt = { status: "opening", error: null, request: {} };
    publish();
    const request = state.prompt.request;
    try {
      const result = await transport.invoke<OpenWorkspaceResult>(COMMANDS.openWorkspace, {
        path,
        scope,
      });
      if (disposed) return;
      // The embedded aggregate is revision-guarded, so even a stale reply may
      // apply; only the latest request owns the prompt and the selection.
      applyState(result.state);
      if (state.prompt.request !== request) return;
      state.prompt.request = null;
      state.prompt.status = "closed";
      state.prompt.error = null;
      selectWorkspace(result.workspace);
      publish();
    } catch (error) {
      if (disposed || state.prompt.request !== request) return;
      state.prompt.request = null;
      state.prompt.status = "editing";
      state.prompt.error = String(error);
      publish();
    }
  }

  function setWorkspacePromptOpen(open: boolean) {
    if (disposed || state.prompt.status === "opening") return;
    state.prompt.status = open ? "editing" : "closed";
    state.prompt.error = null;
    publish();
  }

  /** Registers both event listeners; false means init must not proceed. */
  async function subscribe(): Promise<boolean> {
    try {
      const removeHistory = await transport.listen<void>("invocations-changed", () => {
        if (!disposed) requestHistory();
      });
      if (disposed) {
        removeHistory();
        return false;
      }
      unlistens.push(removeHistory);
      const removeState = await transport.listen<AppState>("app-state", applyState);
      if (disposed) {
        removeState();
        return false;
      }
      unlistens.push(removeState);
      subscribed = true;
      return true;
    } catch (error) {
      unlistens.splice(0).forEach((remove) => remove());
      if (!disposed) {
        state.error = `Could not connect: ${String(error)}`;
        publish();
      }
      return false;
    }
  }

  async function initialize() {
    if (!(await subscribe()) || disposed) return;
    try {
      const snapshot = await transport.invoke<AppState>(COMMANDS.getState);
      if (!disposed) applyState(snapshot);
    } catch (error) {
      if (!disposed && state.core === null) {
        state.error = `Could not connect: ${String(error)}`;
        publish();
      }
    }
  }

  function init(): Promise<void> {
    if (initializing) return initializing;
    if (subscribed || disposed) return Promise.resolve();
    initializing = initialize().finally(() => {
      initializing = undefined;
    });
    return initializing;
  }

  function dispose() {
    disposed = true;
    unlistens.splice(0).forEach((remove) => remove());
  }

  return {
    init,
    dispose,
    start: (cmd: BazelCommand, pattern: string, argumentsText = "") =>
      void start(cmd, pattern, argumentsText),
    stop: (id: string) => void stop(id),
    openWorkspace: (path: string, scope: TargetScope) => void openWorkspace(path, scope),
    setWorkspacePromptOpen,
    refresh: () => void refreshSnapshot(),
    selectInvocation,
    showPage,
    loadDetail: readSavedDetail,
    loadProfile: readSavedProfile,
    loadInvocations: requestHistory,
    select,
  };
}
