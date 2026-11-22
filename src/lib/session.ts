import { COMMANDS } from "./commands.ts";
import { createModules } from "./modules.ts";
import type { Page } from "./pages.ts";
import { targetError } from "./target.ts";
import type {
  AppState,
  BazelCommand,
  ExplorerSelection,
  InvocationDetail,
  InvocationHistory,
  InvocationRecord,
  OpenWorkspaceResult,
  OwnedInvocation,
  ProcessDiagnostic,
  Profile,
  Section,
  ConfigSummary,
  ConfiguredTarget,
  TargetActions,
  TargetDetail,
  TargetIndex,
  TargetLoad,
  TargetOutputs,
  TargetPhaseData,
  TargetReadStep,
  TargetScope,
  TargetTab,
  WorkspaceInfo,
  WorkspaceState,
} from "./types.ts";

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
  /** Static detail: fetched once per selection, independent of tab/flags. */
  detail: Reader<TargetDetail>;
  /** The active non-Definition phase's tagged result; replaced whenever its
   * tab (re)activates or the applied flags change. */
  phase: Reader<TargetPhaseData>;
}

interface Workspace {
  path: string;
  info: Reader<WorkspaceInfo>;
  snapshot: TargetIndex | null;
  snapshotError: string | null;
  refresh: object | null;
  target: TargetSelection;
  targetBusy: boolean;
  /** Inspector tab for the selected target; workspace-scoped so it survives
   * refresh and reselection but resets on an actual workspace switch. */
  targetTab: TargetTab;
  /** Applied build flags for the configured/actions reads; "" is default. */
  targetFlags: string;
  modules: ReturnType<typeof createModules>;
  history: InvocationRecord[];
  historyError: string | null;
  historyBusy: boolean;
  historyDirty: boolean;
  /** This workspace's local diagnostics from the last history reply. */
  diagnostics: ProcessDiagnostic[];
}

interface Saved {
  detail: Reader<InvocationDetail>;
  profile: Reader<Section<Profile>>;
}

interface SessionState {
  core: AppState | null;
  workspace: Workspace | null;
  page: Page;
  selectedInvocationId: string | null;
  start: object | null;
  error: string | null;
  saved: Saved;
}

function reader<T>(value: T | null = null): Reader<T> {
  return { value, error: null, loading: false };
}

function targetSelection(selection: ExplorerSelection = null): TargetSelection {
  return {
    selection,
    detail: reader(),
    phase: reader(),
  };
}

function savedReaders(): Saved {
  return { detail: reader(), profile: reader() };
}

function initialState(page: Page): SessionState {
  return {
    core: null,
    workspace: null,
    page,
    selectedInvocationId: null,
    start: null,
    error: null,
    saved: savedReaders(),
  };
}

function wsEntry(core: AppState | null, path: string | null): WorkspaceState | null {
  if (!core || path === null) return null;
  return core.workspaces.find((entry) => entry.workspace === path) ?? null;
}

function matchesOwner(record: InvocationRecord, execution: OwnedInvocation): boolean {
  return record.id === execution.id && record.workspace === execution.workspace;
}

function visibleExecution(state: SessionState, id: string | null): OwnedInvocation | null {
  const ws = state.workspace;
  const execution = state.core?.invocations.find((entry) => entry.id === id);
  return ws &&
    execution &&
    (execution.workspace === ws.path ||
      ws.history.some((record) => matchesOwner(record, execution)))
    ? execution
    : null;
}

function selectedInvocationId(state: SessionState): string | null {
  return state.page === "history" ? state.selectedInvocationId : null;
}

function savedEligible(state: SessionState): boolean {
  const id = selectedInvocationId(state);
  if (id === null || state.core === null || state.workspace === null) return false;
  const row = invocationRows(state).find((entry) => entry.record.id === id);
  return !!(
    row &&
    !row.execution?.console &&
    !row.diagnostic &&
    state.workspace.history.includes(row.record)
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

/** Live executions owned by the current workspace, newest first. */
function pinnedInvocations(state: SessionState): OwnedInvocation[] {
  const ws = state.workspace;
  return (state.core?.invocations ?? [])
    .filter((entry) => entry.workspace === ws?.path)
    .sort((a, b) => b.startedAt - a.startedAt);
}

/** One History row: a live execution, a saved record, or a local diagnostic.
 * A diagnostic's `record` carries the same fields a saved record would, but
 * never an `exitCode` — its process exit is reported separately. */
interface InvocationRow {
  record: InvocationRecord;
  execution?: OwnedInvocation;
  diagnostic?: ProcessDiagnostic;
}

/** Saved records join the projection in history order: they upgrade a
 * pinned row only when this workspace owns them, and attach a live
 * execution of any workspace the record matches. */
function addSavedRows(state: SessionState, ws: Workspace, rows: Map<string, InvocationRow>) {
  const executions = state.core?.invocations ?? [];
  for (const record of ws.history) {
    const pinned = rows.get(record.id)?.execution;
    if (pinned && !matchesOwner(record, pinned)) continue;
    rows.set(record.id, {
      record,
      execution: pinned ?? executions.find((entry) => matchesOwner(record, entry)),
    });
  }
}

/** This workspace's diagnostics fill ids no same-owner row claims; a
 * foreign same-id record yields to the local diagnostic. */
function addDiagnosticRows(ws: Workspace, rows: Map<string, InvocationRow>) {
  for (const diagnostic of ws.diagnostics) {
    if (diagnostic.workspace !== ws.path) continue;
    if (rows.get(diagnostic.id)?.record.workspace === ws.path) continue;
    rows.set(diagnostic.id, { record: diagnostic, diagnostic });
  }
}

/** The single row projection shared by the view, first-row claiming, and
 * saved-reader eligibility. Pinned live executions keep the head — their
 * local start time can differ from the timestamp a record later reports —
 * and the rest sort newest first with undated rows last. A saved record
 * replaces its pinned row only when this workspace owns it; a diagnostic
 * yields to any same-owner row — live or saved — and takes the place of a
 * foreign same-id record, which is a different build that stays in saved
 * history only. */
function invocationRows(state: SessionState): InvocationRow[] {
  const ws = state.workspace;
  if (!ws) return [];
  const rows = new Map<string, InvocationRow>();
  for (const execution of pinnedInvocations(state)) {
    rows.set(execution.id, { record: execution, execution });
  }
  const pinnedCount = rows.size;
  addSavedRows(state, ws, rows);
  addDiagnosticRows(ws, rows);
  const ordered = [...rows.values()];
  return [
    ...ordered.slice(0, pinnedCount),
    ...ordered
      .slice(pinnedCount)
      .sort((a, b) => (b.record.startedAt ?? -1) - (a.record.startedAt ?? -1)),
  ];
}

/** Flat projection of session state consumed by the Svelte views. */
export type SessionView = ReturnType<typeof sessionView>;

function sessionView(state: SessionState) {
  const eligible = savedEligible(state);
  const core = state.core;
  const ws = state.workspace;
  const history = ws?.history ?? [];
  const rows = invocationRows(state);
  const selected = rows.find((row) => row.record.id === state.selectedInvocationId);
  const target = ws?.target ?? null;
  const saved = state.saved;
  return {
    page: state.page,
    selectedInvocationId: state.selectedInvocationId,
    invocation: state.page === "history" ? (selected ?? null) : null,
    core,
    state: wsEntry(core, ws?.path ?? null),
    workspaceInfo: ws?.info.value ?? null,
    workspaceInfoLoading: ws?.info.loading ?? false,
    workspaceInfoError: ws?.info.error ?? null,
    invocationRows: rows,
    snapshot: ws?.snapshot ?? null,
    snapshotLoading: ws !== null && ws.refresh !== null,
    snapshotError: ws?.snapshotError ?? null,
    selection: target?.selection ?? null,
    targetDetail: target?.detail.value ?? null,
    targetDetailError: target?.detail.error ?? null,
    targetTab: ws?.targetTab ?? "definition",
    targetFlags: ws?.targetFlags ?? "",
    targetPhase: target?.phase.value ?? null,
    targetPhaseError: target?.phase.error ?? null,
    modules: ws?.modules.state ?? null,
    starting: state.start !== null,
    error: state.error,
    invocations: history,
    invocationsError: ws?.historyError ?? null,
    historyLoading: ws?.historyBusy ?? false,
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
    view.state &&
    view.core?.bazel &&
    view.core.besEndpoint &&
    !view.starting &&
    targetError(target) === null
  );
}

export function emptyView(page: Page = "targets"): SessionView {
  return sessionView(initialState(page));
}

export function createSession(
  transport: Transport,
  changed: (view: SessionView) => void,
  page: Page = "targets",
) {
  const state = initialState(page);
  let disposed = false;
  // After an explicit open, saved preferences no longer choose the workspace.
  let workspaceRequest: object | null = null;
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

  /** The active non-Definition phase read; a no-op on Definition or away.
   * Configured stages its three `read_target` steps so a stale context
   * stops between them; leaving the targets page aborts without recording
   * an error and the next visit restarts the read. */
  async function readPhase(
    ws: Workspace,
    target: TargetSelection,
    args: { workspace: string; label: string },
    current: () => boolean,
  ) {
    const phase = ws.targetTab;
    if (state.page !== "targets" || phase === "definition") return;
    const slot = target.phase;
    if (slot.value !== null || slot.error !== null) return;
    const active = () => current() && state.page === "targets";
    const flags = ws.targetFlags;
    const load = <T>(step: TargetReadStep) => {
      if (!active()) throw new Error("Target selection changed");
      return transport.invoke<T>(COMMANDS.readTarget, { ...args, flags, step });
    };
    await read(
      slot,
      async (): Promise<TargetPhaseData> => {
        if (phase === "actions") {
          return { actions: await load<TargetActions>("actions") };
        }
        const configuration = await load<ConfigSummary | null>("configuration");
        const edges = await load<Pick<ConfiguredTarget, "deps" | "depsTruncated">>("transitions");
        const outputs = await load<TargetOutputs>("outputs");
        return { configured: { configuration, outputs, ...edges } };
      },
      active,
    );
  }

  /**
   * The selected target's reads: static detail once per selection, then only
   * the active non-Definition phase while the targets page is showing. Tab
   * and flag changes replace `ws.target`, so every awaited step re-checks
   * that its reader is still current before scheduling another read.
   */
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
      if (target.detail.value === null) {
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
      }
      await readPhase(ws, target, args, current);
    } finally {
      ws.targetBusy = false;
      if (!disposed && state.workspace === ws && ws.target !== target) void loadTarget();
    }
  }

  function acceptSnapshot(ws: Workspace, snapshot: TargetIndex | null) {
    const selection = ws.target.selection;
    let present = false;
    if (snapshot && selection?.kind === "target") {
      present = snapshot.targets.some(({ label }) => label === selection.label);
    } else if (snapshot && selection?.kind === "package") {
      // The tree also contains directories whose only rules are in subpackages.
      present =
        selection.path === "" ||
        snapshot.targets.some(
          ({ pkg }) => pkg === selection.path || pkg.startsWith(`${selection.path}/`),
        );
    }
    ws.snapshot = snapshot;
    ws.snapshotError = null;
    ws.target = targetSelection(present ? selection : null);
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

  /** Claims History's first row while the page sits unselected. */
  function claimFirstInvocation() {
    if (state.page !== "history" || state.selectedInvocationId !== null) return;
    state.selectedInvocationId = invocationRows(state)[0]?.record.id ?? null;
  }

  function acceptHistory(ws: Workspace, result: InvocationHistory, selectedId: string | null) {
    const wasEligible = savedEligible(state);
    if ("Err" in result.saved) {
      // A store failure keeps the last good saved history; this workspace's
      // diagnostics still refresh and saved readers reconcile once.
      ws.diagnostics = result.diagnostics;
      ws.historyError = result.saved.Err;
      claimFirstInvocation();
      reconcileSaved(wasEligible);
      return;
    }
    const records = result.saved.Ok;
    const id = state.selectedInvocationId;
    const next = records.find((record) => record.id === id);
    // Retry with the new selection pinned before replacing its history row.
    if (id !== null && id !== selectedId && next === undefined) {
      ws.historyDirty = true;
      return;
    }
    const previous = ws.history.find((record) => record.id === id);
    const changed = JSON.stringify(previous ?? null) !== JSON.stringify(next ?? null);
    const detail = state.saved.detail;
    const detailMismatch =
      next !== undefined &&
      detail.value !== null &&
      !detail.loading &&
      detail.error === null &&
      JSON.stringify(next) !== JSON.stringify(detail.value.record);
    ws.diagnostics = result.diagnostics;
    ws.history = records;
    ws.historyError = null;
    // An unselected history page claims its first row; reconcileSaved below
    // starts the saved readers exactly once for the new selection.
    claimFirstInvocation();
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
    const selectedId = state.selectedInvocationId;
    publish();
    const current = () => !disposed && state.workspace === ws;
    try {
      const result = await transport.invoke<InvocationHistory>(COMMANDS.listInvocations, {
        workspace: ws.path,
        selectedId,
      });
      if (current()) acceptHistory(ws, result, selectedId);
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
    if (state.selectedInvocationId === null) return;
    if (state.page !== "history") {
      state.saved = savedReaders();
      return;
    }
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

  function loadWorkspaceInfo() {
    const ws = state.workspace;
    if (disposed || !ws || !state.core?.bazel || !wsEntry(state.core, ws.path)) return;
    const current = () => !disposed && state.workspace === ws;
    void read(
      ws.info,
      () => transport.invoke<WorkspaceInfo>(COMMANDS.getWorkspaceInfo, { workspace: ws.path }),
      current,
    );
  }

  function selectWorkspace(path: string) {
    if (disposed || state.workspace?.path === path) return;
    state.workspace?.modules.dispose();
    state.workspace = {
      path,
      info: reader(),
      snapshot: null,
      snapshotError: null,
      refresh: null,
      target: targetSelection(),
      targetBusy: false,
      targetTab: "definition",
      targetFlags: "",
      modules: createModules(path, transport, () => state.core?.bazel != null, publish),
      history: [],
      historyError: null,
      historyBusy: false,
      historyDirty: false,
      diagnostics: [],
    };
    state.selectedInvocationId = null;
    state.start = null;
    state.error = null;
    state.saved = savedReaders();
    publish();
    requestHistory();
    void refreshSnapshot();
    if (state.page === "workspace") loadWorkspaceInfo();
  }

  function openInvocation(id: string, lookup: boolean) {
    if (disposed) return;
    const previousId = selectedInvocationId(state);
    state.page = "history";
    state.selectedInvocationId = id;
    state.start = null;
    if (previousId !== id) {
      state.saved = savedReaders();
    }
    publish();
    ensureSaved();
    const saved = state.saved;
    if (!savedEligible(state)) {
      // An id the current history cannot show may still exist past the latest
      // window; only a user-driven selection pins it through a fresh list
      // read. savedEligible already scanned history, so this probe runs only
      // when the record is absent.
      if (
        lookup &&
        state.workspace !== null &&
        !visibleExecution(state, id) &&
        !state.workspace.history.some((record) => record.id === id)
      ) {
        requestHistory();
      }
      return;
    }
    // Re-selecting the same invocation only retries failed readers.
    if (saved.detail.error !== null) readSavedDetail();
    if (saved.profile.error !== null) readSavedProfile();
  }

  function selectInvocation(id: string) {
    openInvocation(id, true);
  }

  function maybeLoadWorkspaceInfo() {
    const ws = state.workspace;
    if (ws && ws.info.value === null && ws.info.error === null && !ws.info.loading) {
      loadWorkspaceInfo();
    }
  }

  function showPage(page: Page) {
    if (disposed) return;
    const changed = state.page !== page;
    state.page = page;
    state.start = null;
    if (changed) {
      // Claim History's first row before the frame so it never flashes empty.
      claimFirstInvocation();
      publish();
      if (page === "history") ensureSaved();
    }
    if (page === "workspace") maybeLoadWorkspaceInfo();
    // Returning to Targets kicks any phase read deferred while away.
    if (page === "targets") void loadTarget();
  }

  /** Replaces the phase reader; loadTarget reloads only the active one. */
  function reloadTargetPhase(ws: Workspace) {
    ws.target = { ...ws.target, phase: reader() };
    publish();
    void loadTarget();
  }

  /** Switches the inspector tab; phase readers restart, detail is kept. */
  function setTargetTab(tab: TargetTab) {
    if (disposed) return;
    const ws = state.workspace;
    if (!ws || ws.targetTab === tab) return;
    ws.targetTab = tab;
    reloadTargetPhase(ws);
  }

  /** Applies build flags; the active non-Definition phase reloads, even
   * for unchanged flags. Definition keeps its static read. Malformed or
   * reserved flags are rejected by the backend before any Bazel process
   * spawns and surface as the phase error. */
  function applyTargetFlags(flags: string) {
    const ws = state.workspace;
    if (disposed || !ws) return;
    ws.targetFlags = flags;
    reloadTargetPhase(ws);
  }

  function select(selection: ExplorerSelection) {
    if (disposed) return;
    state.page = "targets";
    state.start = null;
    const ws = state.workspace;
    if (!ws) {
      publish();
      return;
    }
    const target = ws.target;
    // A new selection always replaces the target context; reselecting the
    // same target only retries a failed detail read. Either way the shared
    // tail publishes and lets loadTarget kick any deferred phase read.
    if (
      !sameSelection(target.selection, selection) ||
      (selection?.kind === "target" && target.detail.error !== null)
    ) {
      ws.target = targetSelection(selection);
    } else {
      // Same pick again: hand the view a fresh selection object so its
      // effects observe the re-pick while the readers stay cached.
      target.selection = selection;
    }
    publish();
    void loadTarget();
  }

  function restorableWorkspace(snapshot: AppState): string | null {
    if (
      workspaceRequest !== null ||
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
    // A live invocation arriving while History sits unselected claims the
    // first row; reconcileSaved starts its readers once.
    claimFirstInvocation();
    reconcileSaved(wasEligible);
    publish();
  }

  async function start(cmd: BazelCommand, pattern: string, argumentsText: string) {
    const ws = state.workspace;
    if (disposed || !ws || !canSend(sessionView(state), pattern)) return;
    const token = {};
    const openRequest = workspaceRequest;
    state.start = token;
    state.error = null;
    publish();
    try {
      const id = await transport.invoke<string>(COMMANDS.startInvocation, {
        workspace: ws.path,
        cmd,
        pattern,
        arguments: argumentsText,
      });
      if (disposed || state.start !== token) return;
      state.start = null;
      if (workspaceRequest !== openRequest || state.workspace !== ws) {
        publish();
        return;
      }
      openInvocation(id, false);
    } catch (error) {
      if (disposed || state.start !== token) return;
      state.start = null;
      if (workspaceRequest !== openRequest || state.workspace !== ws) {
        publish();
        return;
      }
      state.error = String(error);
      publish();
    }
  }

  async function stop(id: string) {
    if (disposed) return;
    const ws = state.workspace;
    const execution = visibleExecution(state, id);
    if (!execution?.stoppable || execution.stopping) return;
    try {
      await transport.invoke<void>(COMMANDS.stopInvocation, {
        workspace: execution.workspace,
        id: execution.id,
      });
    } catch (error) {
      if (disposed) return;
      if (state.workspace === ws && selectedInvocationId(state) === id) {
        state.error = String(error);
        publish();
      }
    }
  }

  /**
   * Opens a workspace. Resolves true when this request selected the canonical
   * workspace, false when a newer request or disposal superseded it, and
   * rejects only when the current request itself failed.
   */
  async function openWorkspace(path: string, scope: TargetScope): Promise<boolean> {
    if (disposed) return false;
    const request = {};
    workspaceRequest = request;
    try {
      const result = await transport.invoke<OpenWorkspaceResult>(COMMANDS.openWorkspace, {
        path,
        scope,
      });
      if (disposed) return false;
      // The embedded aggregate is revision-guarded, so even a stale reply may
      // apply; only the latest request owns the selection.
      applyState(result.state);
      if (workspaceRequest !== request) return false;
      selectWorkspace(result.workspace);
      publish();
      return true;
    } catch (error) {
      if (disposed || workspaceRequest !== request) return false;
      throw error;
    }
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

  /** Runs once per mounted session; the root pairs it with dispose. */
  async function init() {
    if (disposed) return;
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

  function dispose() {
    disposed = true;
    state.workspace?.modules.dispose();
    unlistens.splice(0).forEach((remove) => remove());
  }

  return {
    init,
    dispose,
    start: (cmd: BazelCommand, pattern: string, argumentsText = "") =>
      void start(cmd, pattern, argumentsText),
    stop: (id: string) => void stop(id),
    openWorkspace,
    loadWorkspaceInfo,
    refresh: () => void refreshSnapshot(),
    loadModules: () => state.workspace?.modules.load(),
    selectModule: (key: string | null) => state.workspace?.modules.select(key),
    explainModule: () => state.workspace?.modules.explain(),
    selectInvocation,
    showPage,
    loadDetail: readSavedDetail,
    loadProfile: readSavedProfile,
    loadInvocations: requestHistory,
    select,
    setTargetTab,
    applyTargetFlags,
  };
}
