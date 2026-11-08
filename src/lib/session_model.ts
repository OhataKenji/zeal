import { targetError } from "./target.ts";
import type {
  AppState,
  BazelCommand,
  ExplorerSelection,
  InvocationDetail,
  InvocationRecord,
  Profile,
  Section,
  TargetActions,
  TargetDetail,
  TargetIndex,
  TargetOutputs,
  TargetScope,
} from "./types.ts";

export type WorkspacePage = "targets" | "analytics" | "query";

export type SessionContent =
  | { readonly kind: WorkspacePage }
  | { readonly kind: "invocation"; readonly id: string; readonly page: WorkspacePage }
  | { readonly kind: "console"; readonly page: WorkspacePage };

interface Reader<T> {
  value: T | null;
  error: string | null;
  running: number | null;
  wanted: number | null;
}

interface HistoryRequest {
  id: number;
  status: "current" | "refresh" | "invalidated";
}

interface ObservedExecution {
  readonly id: string;
  readonly active: boolean;
}

interface StartRequest {
  readonly id: number;
  readonly cmd: BazelCommand;
  readonly pattern: string;
  readonly previousExecution: ObservedExecution | null;
  readonly observedId: string | null;
  readonly recordId: string | null;
}

export interface SessionModel {
  readonly content: SessionContent;
  readonly connection: {
    state: AppState | null;
    ready: boolean;
    revision: number;
    error: string | null;
  };
  readonly workspacePrompt: {
    status: "closed" | "editing" | "opening";
    error: string | null;
  };
  readonly execution: {
    console: InvocationRecord | null;
    pendingStart: StartRequest | null;
    observed: ObservedExecution | null;
  };
  readonly targets: {
    selection: ExplorerSelection;
    snapshot: {
      value: TargetIndex | null;
      error: string | null;
      requestId: number | null;
    };
    detail: Reader<TargetDetail>;
    outputs: Reader<TargetOutputs>;
    actions: Reader<TargetActions>;
  };
  readonly history: {
    records: InvocationRecord[];
    error: string | null;
    request: HistoryRequest | null;
  };
  readonly saved: {
    detail: Reader<InvocationDetail>;
    profile: Reader<Section<Profile>>;
  };
  readonly nextRequestId: number;
}

export interface SessionView {
  content: SessionContent;
  console: InvocationRecord | null;
  state: AppState | null;
  snapshot: TargetIndex | null;
  snapshotLoading: boolean;
  snapshotError: string | null;
  selection: ExplorerSelection;
  targetDetail: TargetDetail | null;
  targetDetailError: string | null;
  targetOutputs: TargetOutputs | null;
  targetOutputsError: string | null;
  targetActions: TargetActions | null;
  targetActionsError: string | null;
  ready: boolean;
  starting: boolean;
  opening: boolean;
  workspacePromptOpen: boolean;
  workspaceError: string | null;
  error: string | null;
  invocations: InvocationRecord[];
  invocationsError: string | null;
  detail: InvocationDetail | null;
  detailLoading: boolean;
  detailError: string | null;
  profile: Section<Profile> | null;
  profileLoading: boolean;
  profileError: string | null;
}

export type Result<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: string };

type ResultMessage<T extends string, V> = {
  readonly type: T;
  readonly result: Result<V>;
};
type RequestResult<T extends string, V> = ResultMessage<T, V> & {
  readonly requestId: number;
};

export type SessionEffect =
  | { readonly type: "listInvocations"; readonly id: number }
  | { readonly type: "getSnapshot"; readonly id: number }
  | {
      readonly type: "getTargetDetail";
      readonly id: number;
      readonly label: string;
      readonly fetchedAt: number;
    }
  | {
      readonly type: "getTargetOutputs";
      readonly id: number;
      readonly label: string;
      readonly fetchedAt: number;
    }
  | {
      readonly type: "getTargetActions";
      readonly id: number;
      readonly label: string;
      readonly fetchedAt: number;
    }
  | { readonly type: "getInvocation"; readonly id: number; readonly invocationId: string }
  | { readonly type: "getProfile"; readonly id: number; readonly invocationId: string }
  | {
      readonly type: "startInvocation";
      readonly id: number;
      readonly cmd: BazelCommand;
      readonly pattern: string;
      readonly argumentsText: string;
    }
  | {
      readonly type: "getStartState";
      readonly startId: number;
      readonly revision: number;
    }
  | {
      readonly type: "openWorkspace";
      readonly path: string;
      readonly scope: TargetScope;
      readonly revision: number;
    }
  | { readonly type: "stop"; readonly workspace: string | null }
  | { readonly type: "refresh"; readonly workspace: string | null };

export type SessionMessage =
  | { readonly type: "subscriptionsFailed"; readonly error: string }
  | (ResultMessage<"initialStateResult", AppState> & { readonly revision: number })
  | { readonly type: "appState"; readonly state: AppState }
  | { readonly type: "historyChanged" }
  | { readonly type: "loadInvocations" }
  | RequestResult<"invocationsResult", InvocationRecord[]>
  | { readonly type: "loadSnapshot" }
  | RequestResult<"snapshotResult", TargetIndex | null>
  | RequestResult<"targetDetailResult", TargetDetail>
  | RequestResult<"targetOutputsResult", TargetOutputs>
  | RequestResult<"targetActionsResult", TargetActions>
  | RequestResult<"detailResult", InvocationDetail>
  | RequestResult<"profileResult", Section<Profile>>
  | { readonly type: "select"; readonly selection: ExplorerSelection }
  | { readonly type: "selectInvocation"; readonly id: string }
  | { readonly type: "showPage"; readonly page: WorkspacePage }
  | { readonly type: "openConsole"; readonly record: InvocationRecord }
  | { readonly type: "loadDetail" }
  | { readonly type: "loadProfile" }
  | {
      readonly type: "start";
      readonly cmd: BazelCommand;
      readonly pattern: string;
      readonly argumentsText: string;
    }
  | RequestResult<"startResult", InvocationRecord>
  | (ResultMessage<"startStateResult", AppState> & {
      readonly startId: number;
      readonly revision: number;
    })
  | { readonly type: "workspacePrompt"; readonly open: boolean }
  | { readonly type: "openWorkspace"; readonly path: string; readonly scope: TargetScope }
  | (ResultMessage<"workspaceResult", AppState> & { readonly revision: number })
  | { readonly type: "stop" }
  | (ResultMessage<"stopResult", void> & { readonly workspace: string | null })
  | { readonly type: "refresh" }
  | (ResultMessage<"refreshResult", void> & { readonly workspace: string | null });

interface SessionTransition {
  readonly model: SessionModel;
  readonly effects: SessionEffect[];
}

const RUNNING_SLOTS = ["ingest", "detailQuery"] as const;
const TARGET_LIST_ERROR = "Could not load the target list";

function emptyReader<T>(): Reader<T> {
  return { value: null, error: null, running: null, wanted: null };
}

export function initialModel(page: WorkspacePage = "targets"): SessionModel {
  return {
    content: { kind: page },
    connection: { state: null, ready: false, revision: 0, error: null },
    workspacePrompt: { status: "closed", error: null },
    execution: { console: null, pendingStart: null, observed: null },
    targets: {
      selection: null,
      snapshot: { value: null, error: null, requestId: null },
      detail: emptyReader<TargetDetail>(),
      outputs: emptyReader<TargetOutputs>(),
      actions: emptyReader<TargetActions>(),
    },
    history: { records: [], error: null, request: null },
    saved: {
      detail: emptyReader<InvocationDetail>(),
      profile: emptyReader<Section<Profile>>(),
    },
    nextRequestId: 1,
  };
}

function selectedInvocation(model: SessionModel): string | null {
  const content = model.content;
  return content.kind === "invocation" ? content.id : null;
}

function workspacePage(model: SessionModel): WorkspacePage {
  const content = model.content;
  return content.kind === "invocation" || content.kind === "console" ? content.page : content.kind;
}

function savedEligible(model: SessionModel): boolean {
  const id = selectedInvocation(model);
  return !!(
    id &&
    model.connection.ready &&
    model.connection.state?.workspace &&
    model.execution.pendingStart === null &&
    id !== model.connection.state.activeInvocation
  );
}

export function sessionView(model: SessionModel): SessionView {
  const eligible = savedEligible(model);
  return {
    content: model.content,
    console: model.execution.console,
    state: model.connection.state,
    snapshot: model.targets.snapshot.value,
    snapshotLoading: model.targets.snapshot.requestId !== null,
    snapshotError: model.targets.snapshot.error,
    selection: model.targets.selection,
    targetDetail: model.targets.detail.value,
    targetDetailError: model.targets.detail.error,
    targetOutputs: model.targets.outputs.value,
    targetOutputsError: model.targets.outputs.error,
    targetActions: model.targets.actions.value,
    targetActionsError: model.targets.actions.error,
    ready: model.connection.ready,
    starting: model.execution.pendingStart !== null,
    opening: model.workspacePrompt.status === "opening",
    workspacePromptOpen: model.workspacePrompt.status !== "closed",
    workspaceError: model.workspacePrompt.error,
    error: model.connection.error,
    invocations: model.history.records,
    invocationsError: model.history.error,
    detail: model.saved.detail.value,
    detailError: model.saved.detail.error,
    profile: model.saved.profile.value,
    detailLoading:
      eligible &&
      model.saved.detail.running !== null &&
      model.saved.detail.running === model.saved.detail.wanted,
    profileLoading:
      eligible &&
      model.saved.profile.running !== null &&
      model.saved.profile.running === model.saved.profile.wanted,
    profileError: model.saved.profile.error,
  };
}

export function emptyView(page: WorkspacePage = "targets"): SessionView {
  return sessionView(initialModel(page));
}

export function bazelBusyReason(state: AppState | null): string | null {
  if (state?.activeInvocation) return "a Bazel operation is in progress";
  const reasons = {
    ingest: "targets are loading",
    detailQuery: "target details are loading",
  };
  const slot = RUNNING_SLOTS.find((name) => state?.[name] === "running");
  return slot ? reasons[slot] : null;
}

function workspaceReady(view: SessionView): boolean {
  return !!(view.state?.workspace && view.state.bazel);
}

export function sessionBusy(view: SessionView): boolean {
  return !view.ready || view.opening || view.starting || bazelBusyReason(view.state) !== null;
}

export function canSend(view: SessionView, target: string): boolean {
  return !!(!sessionBusy(view) && workspaceReady(view) && targetError(target) === null);
}

export function canStop(view: SessionView): boolean {
  return !!(view.ready && view.state?.activeInvocation && !view.state.stopping);
}

type Draft = { -readonly [K in keyof SessionModel]: SessionModel[K] } & {
  effects: SessionEffect[];
};

function draft(model: SessionModel): Draft {
  // Only these fresh containers are mutable; nested requests and payloads stay shared.
  return {
    ...model,
    connection: { ...model.connection },
    workspacePrompt: { ...model.workspacePrompt },
    execution: { ...model.execution },
    targets: {
      ...model.targets,
      snapshot: { ...model.targets.snapshot },
      detail: { ...model.targets.detail },
      outputs: { ...model.targets.outputs },
      actions: { ...model.targets.actions },
    },
    history: {
      ...model.history,
      request: model.history.request ? { ...model.history.request } : null,
    },
    saved: {
      ...model.saved,
      detail: { ...model.saved.detail },
      profile: { ...model.saved.profile },
    },
    effects: [],
  };
}

function finishDraft(next: Draft): SessionTransition {
  const { effects, ...model } = next;
  return { model, effects };
}

function nextId(draft: Draft): number {
  return draft.nextRequestId++;
}

function unexpected(message: never): never {
  throw new Error(`Unhandled session message: ${JSON.stringify(message)}`);
}

function sameScope(left: AppState | null, right: AppState): boolean {
  return JSON.stringify(left?.scope.include ?? []) === JSON.stringify(right.scope.include);
}

function sameSelection(left: ExplorerSelection, right: ExplorerSelection): boolean {
  if (left?.kind !== right?.kind) return false;
  if (left?.kind === "target" && right?.kind === "target") return left.label === right.label;
  if (left?.kind === "package" && right?.kind === "package") return left.path === right.path;
  return left === null && right === null;
}

function recordFor(records: InvocationRecord[], id: string | null): InvocationRecord | undefined {
  return records.find((record) => record.id === id);
}

function startHistory(draft: Draft) {
  if (!draft.connection.ready || !draft.connection.state?.workspace || draft.history.request) {
    return;
  }
  const id = nextId(draft);
  draft.history.request = { id, status: "current" };
  draft.history.error = null;
  draft.effects.push({ type: "listInvocations", id });
}

function requestHistory(draft: Draft) {
  if (draft.history.request) {
    if (draft.history.request.status === "current") {
      draft.history.request.status = "refresh";
    }
  } else {
    startHistory(draft);
  }
}

function startSnapshot(draft: Draft) {
  const state = draft.connection.state;
  if (!state?.workspace || state.snapshotFetchedAt === null) return;
  if (draft.targets.snapshot.requestId !== null) return;
  const id = nextId(draft);
  draft.targets.snapshot.requestId = id;
  draft.targets.snapshot.error = null;
  draft.effects.push({ type: "getSnapshot", id });
}

function targetReadArgs(draft: Draft): { label: string; fetchedAt: number } | null {
  const selection = draft.targets.selection;
  const state = draft.connection.state;
  const snapshot = draft.targets.snapshot.value;
  const view = sessionView(draft);
  if (
    selection?.kind !== "target" ||
    !snapshot ||
    !snapshot.targets.some((target) => target.label === selection.label) ||
    snapshot.meta.fetchedAt !== state?.snapshotFetchedAt ||
    !workspaceReady(view) ||
    sessionBusy(view)
  )
    return null;
  return { label: selection.label, fetchedAt: snapshot.meta.fetchedAt };
}

function startTargetDetail(draft: Draft) {
  const { detail, outputs, actions } = draft.targets;
  if (
    detail.running !== null ||
    detail.wanted === null ||
    outputs.running !== null ||
    actions.running !== null
  )
    return;
  const args = targetReadArgs(draft);
  if (!args) return;
  const id = detail.wanted;
  detail.running = id;
  draft.effects.push({ type: "getTargetDetail", id, ...args });
}

function startTargetOutputs(draft: Draft) {
  const { detail, outputs, actions } = draft.targets;
  if (
    outputs.running !== null ||
    outputs.wanted === null ||
    detail.running !== null ||
    actions.running !== null
  )
    return;
  const args = targetReadArgs(draft);
  if (!args || detail.value?.target.label !== args.label) return;
  const id = outputs.wanted;
  outputs.running = id;
  draft.effects.push({ type: "getTargetOutputs", id, ...args });
}

function startTargetActions(draft: Draft) {
  const { detail, outputs, actions } = draft.targets;
  if (
    actions.running !== null ||
    actions.wanted === null ||
    detail.running !== null ||
    detail.wanted !== null ||
    outputs.running !== null ||
    outputs.wanted !== null ||
    (outputs.value === null && outputs.error === null)
  )
    return;
  const args = targetReadArgs(draft);
  if (!args || detail.value?.target.label !== args.label) return;
  const id = actions.wanted;
  actions.running = id;
  draft.effects.push({ type: "getTargetActions", id, ...args });
}

function startDetail(draft: Draft) {
  if (
    draft.saved.detail.running !== null ||
    draft.saved.detail.wanted === null ||
    !savedEligible(draft)
  )
    return;
  const invocationId = selectedInvocation(draft);
  if (!invocationId) return;
  const id = draft.saved.detail.wanted;
  draft.saved.detail.running = id;
  draft.saved.detail.error = null;
  draft.effects.push({ type: "getInvocation", id, invocationId });
}

function startProfile(draft: Draft) {
  if (
    draft.saved.profile.running !== null ||
    draft.saved.profile.wanted === null ||
    !savedEligible(draft)
  )
    return;
  const invocationId = selectedInvocation(draft);
  if (!invocationId) return;
  const id = draft.saved.profile.wanted;
  draft.saved.profile.running = id;
  draft.saved.profile.error = null;
  draft.effects.push({ type: "getProfile", id, invocationId });
}

function resumeTargets(draft: Draft) {
  startTargetDetail(draft);
  startTargetOutputs(draft);
  startTargetActions(draft);
}

function resumeSaved(draft: Draft) {
  startDetail(draft);
  startProfile(draft);
}

function retryDetail(draft: Draft) {
  if (selectedInvocation(draft) === null) return;
  if (draft.saved.detail.wanted === null) draft.saved.detail.wanted = nextId(draft);
  startDetail(draft);
}

function retryProfile(draft: Draft) {
  if (selectedInvocation(draft) === null) return;
  if (draft.saved.profile.wanted === null) draft.saved.profile.wanted = nextId(draft);
  startProfile(draft);
}

function sameContent(left: SessionContent, right: SessionContent): boolean {
  if (left.kind !== right.kind) return false;
  if (left.kind === "invocation" && right.kind === "invocation") {
    return left.id === right.id && left.page === right.page;
  }
  if (left.kind === "console" && right.kind === "console") return left.page === right.page;
  return true;
}

function showContent(draft: Draft, content: SessionContent) {
  if (sameContent(draft.content, content)) return;
  const previousId = selectedInvocation(draft);
  draft.content = content;
  const id = selectedInvocation(draft);
  if (previousId === id) return;
  draft.saved.detail.value = null;
  draft.saved.detail.error = null;
  draft.saved.profile.value = null;
  draft.saved.profile.error = null;
  draft.saved.detail.wanted = id === null ? null : nextId(draft);
  draft.saved.profile.wanted = id === null ? null : nextId(draft);
  resumeSaved(draft);
}

function selectSaved(draft: Draft, id: string) {
  showContent(draft, { kind: "invocation", id, page: workspacePage(draft) });
  retryDetail(draft);
  retryProfile(draft);
}

function showPage(draft: Draft, page: WorkspacePage) {
  showContent(draft, { kind: page });
}

function savedEligibilityChanged(draft: Draft, wasEligible: boolean, isEligible: boolean) {
  if (wasEligible === isEligible) return;
  if (isEligible) {
    resumeSaved(draft);
    return;
  }
  if (selectedInvocation(draft) === null) {
    draft.saved.detail.wanted = null;
    draft.saved.profile.wanted = null;
    return;
  }
  revokeReaderIntent(draft, draft.saved.detail);
  revokeReaderIntent(draft, draft.saved.profile);
}

function revokeReaderIntent<T>(draft: Draft, reader: Reader<T>) {
  if (reader.running !== null && reader.wanted === reader.running) {
    reader.wanted = nextId(draft);
  }
}

function refreshSavedFromHistory(draft: Draft) {
  if (selectedInvocation(draft) === null) return;
  draft.saved.detail.wanted = nextId(draft);
  draft.saved.profile.wanted = nextId(draft);
  draft.saved.detail.error = null;
  draft.saved.profile.error = null;
  resumeSaved(draft);
}

function queueTargetDetail(draft: Draft) {
  if (draft.targets.selection?.kind !== "target") {
    draft.targets.detail.wanted = null;
    draft.targets.outputs.wanted = null;
    draft.targets.actions.wanted = null;
    return;
  }
  draft.targets.detail.wanted = nextId(draft);
  draft.targets.outputs.wanted = null;
  draft.targets.actions.wanted = null;
}

function resetTargetContext(draft: Draft) {
  draft.targets.detail.value = null;
  draft.targets.detail.error = null;
  draft.targets.outputs.value = null;
  draft.targets.outputs.error = null;
  draft.targets.actions.value = null;
  draft.targets.actions.error = null;
  queueTargetDetail(draft);
  resumeTargets(draft);
}

function selectTarget(draft: Draft, selection: ExplorerSelection) {
  if (sameSelection(draft.targets.selection, selection)) {
    if (selection?.kind === "target" && draft.targets.detail.error !== null) {
      draft.targets.detail.error = null;
      queueTargetDetail(draft);
      startTargetDetail(draft);
    }
    return;
  }
  draft.targets.selection = selection;
  resetTargetContext(draft);
}

function resetWorkspaceData(draft: Draft, state: AppState) {
  draft.execution.pendingStart = null;
  draft.execution.observed = state.activeInvocation
    ? { id: state.activeInvocation, active: true }
    : null;
  draft.targets.snapshot.requestId = null;
  if (draft.history.request) draft.history.request.status = "invalidated";
  showPage(draft, workspacePage(draft));
  draft.targets.selection = null;
  resetTargetContext(draft);
  draft.execution.console = null;
  draft.targets.snapshot.value = null;
  draft.targets.snapshot.error = null;
  draft.history.records = [];
  draft.history.error = null;
}

function applyPendingStartState(draft: Draft, state: AppState): boolean {
  const starting = draft.execution.pendingStart;
  const activeId = state.activeInvocation;
  if (
    !starting ||
    !activeId ||
    state.consoleInvocation !== activeId ||
    starting.observedId === activeId
  ) {
    return false;
  }
  draft.execution.pendingStart = { ...starting, observedId: activeId };
  draft.execution.console = { id: activeId, cmd: starting.cmd, pattern: starting.pattern };
  showContent(draft, { kind: "console", page: workspacePage(draft) });
  return true;
}

function observedCompletionId(
  draft: Draft,
  before: AppState | null,
  state: AppState,
  activeId: string | null,
): string | null {
  const observed = draft.execution.observed;
  if (activeId) {
    return observed?.active && before?.activeInvocation === observed.id && observed.id !== activeId
      ? observed.id
      : null;
  }
  if (!observed) return null;
  if (
    (observed.active && before?.activeInvocation === observed.id) ||
    state.consoleInvocation === observed.id
  ) {
    return observed.id;
  }
  return null;
}

function observeExecution(draft: Draft, before: AppState | null, state: AppState) {
  const activeId = state.activeInvocation;
  const completed = observedCompletionId(draft, before, state, activeId);
  draft.execution.observed = activeId ? { id: activeId, active: true } : null;
  if (completed) completeConsole(draft, completed);
}

function updateExecutionState(draft: Draft, before: AppState | null, state: AppState) {
  if (!applyPendingStartState(draft, state)) observeExecution(draft, before, state);
}

function updateTargetsForState(draft: Draft, before: AppState | null, state: AppState) {
  const workspaceChanged = state.workspace !== before?.workspace;
  const scopeChanged = !sameScope(before, state);
  const contextChanged =
    before === null ||
    workspaceChanged ||
    scopeChanged ||
    state.snapshotFetchedAt !== before.snapshotFetchedAt;
  let selectionCleared = false;
  if (!workspaceChanged && (scopeChanged || state.snapshotFetchedAt === null)) {
    selectionCleared = draft.targets.selection !== null;
    draft.targets.selection = null;
    draft.targets.snapshot.value = null;
    draft.targets.snapshot.error = null;
  }
  if ((contextChanged && !workspaceChanged) || selectionCleared) resetTargetContext(draft);
  if (!contextChanged) {
    if (draft.targets.snapshot.error !== null) startSnapshot(draft);
    return;
  }
  draft.targets.snapshot.requestId = null;
  if (state.snapshotFetchedAt === null || scopeChanged || workspaceChanged) {
    draft.targets.snapshot.value = null;
  }
  startSnapshot(draft);
}

function applyAppState(draft: Draft, state: AppState) {
  const before = draft.connection.state;
  const wasReady = draft.connection.ready;
  const wasSavedEligible = savedEligible(draft);
  const wasBusy = sessionBusy(sessionView(draft));
  draft.connection.revision++;
  draft.connection.state = state;
  draft.connection.ready = true;
  if (!wasReady) draft.connection.error = null;
  if (state.workspace !== before?.workspace) {
    resetWorkspaceData(draft, state);
    startHistory(draft);
  } else {
    updateExecutionState(draft, before, state);
    if (!wasReady || state.activeInvocation !== before?.activeInvocation) requestHistory(draft);
  }
  updateTargetsForState(draft, before, state);
  savedEligibilityChanged(draft, wasSavedEligible, savedEligible(draft));
  if (wasBusy && sessionBusy(sessionView(draft)) === false) resumeTargets(draft);
}

function completeConsole(draft: Draft, id: string) {
  if (
    draft.execution.pendingStart ||
    draft.connection.state?.activeInvocation ||
    draft.content.kind !== "console" ||
    draft.execution.console?.id !== id
  ) {
    return;
  }
  selectSaved(draft, id);
}

function finishStart(draft: Draft, requestId: number) {
  const current = draft.execution.pendingStart;
  if (!current || current.id !== requestId) return;
  const wasEligible = savedEligible(draft);
  draft.execution.pendingStart = null;
  if (current.recordId === null) {
    draft.execution.observed = current.previousExecution;
  } else {
    const id = current.recordId;
    if (!draft.connection.ready) {
      draft.execution.observed = { id, active: current.observedId === id };
    } else if (draft.connection.state?.activeInvocation === id) {
      draft.execution.observed = { id, active: true };
    } else if (!draft.connection.state?.activeInvocation) {
      draft.execution.observed = null;
      completeConsole(draft, id);
    } else {
      draft.execution.observed = { id: draft.connection.state.activeInvocation, active: true };
    }
  }
  savedEligibilityChanged(draft, wasEligible, savedEligible(draft));
  resumeTargets(draft);
}

function acceptHistory(draft: Draft, records: InvocationRecord[]) {
  const id = selectedInvocation(draft);
  const previous = recordFor(draft.history.records, id);
  const next = recordFor(records, id);
  const changed = JSON.stringify(previous ?? null) !== JSON.stringify(next ?? null);
  draft.history.records = records;
  draft.history.error = null;
  const detailMismatch =
    next !== undefined &&
    draft.saved.detail.value !== null &&
    draft.saved.detail.running === null &&
    draft.saved.detail.error === null &&
    JSON.stringify(next) !== JSON.stringify(draft.saved.detail.value.record);
  if (changed || detailMismatch) refreshSavedFromHistory(draft);
}

function finishHistory(draft: Draft, requestId: number, result: Result<InvocationRecord[]>) {
  const current = draft.history.request;
  if (!current || current.id !== requestId) return;
  draft.history.request = null;
  if (current.status !== "invalidated") {
    if (result.ok) acceptHistory(draft, result.value);
    else draft.history.error = result.error;
  }
  if (current.status !== "current") startHistory(draft);
}

function acceptSnapshot(draft: Draft, snapshot: TargetIndex | null) {
  const previousFetchedAt = draft.targets.snapshot.value?.meta.fetchedAt ?? null;
  const selection = draft.targets.selection;
  const validTarget =
    selection?.kind === "target" &&
    snapshot?.targets.some((target) => target.label === selection.label);
  const selectionInvalid =
    selection?.kind === "package" || (selection?.kind === "target" && !validTarget);
  draft.targets.snapshot.value = snapshot;
  draft.targets.snapshot.error = null;
  if (draft.targets.detail.error === TARGET_LIST_ERROR) {
    draft.targets.detail.error = null;
  }
  if (selectionInvalid) draft.targets.selection = null;
  if (previousFetchedAt !== (snapshot?.meta.fetchedAt ?? null) || selectionInvalid) {
    resetTargetContext(draft);
  }
  resumeTargets(draft);
}

function failSnapshot(draft: Draft, error: string) {
  draft.targets.snapshot.error = `Could not load the workspace snapshot: ${error}`;
  if (draft.targets.selection?.kind === "target") draft.targets.detail.error = TARGET_LIST_ERROR;
}

function finishSnapshot(draft: Draft, requestId: number, result: Result<TargetIndex | null>) {
  if (draft.targets.snapshot.requestId !== requestId) return;
  draft.targets.snapshot.requestId = null;
  if (result.ok) acceptSnapshot(draft, result.value);
  else failSnapshot(draft, result.error);
}

function acceptTargetDetail(draft: Draft, result: Result<TargetDetail>) {
  const selection = draft.targets.selection;
  if (!result.ok) {
    draft.targets.detail.error = result.error;
    return;
  }
  if (selection?.kind !== "target" || result.value.target.label !== selection.label) {
    draft.targets.detail.error = "Error: Target detail does not match the selected label";
    return;
  }
  draft.targets.detail.value = result.value;
  draft.targets.detail.error = null;
  if (
    draft.targets.outputs.value === null &&
    draft.targets.outputs.error === null &&
    draft.targets.outputs.wanted === null
  ) {
    draft.targets.outputs.wanted = nextId(draft);
  }
  if (
    draft.targets.actions.value === null &&
    draft.targets.actions.error === null &&
    draft.targets.actions.wanted === null
  ) {
    draft.targets.actions.wanted = nextId(draft);
  }
  startTargetOutputs(draft);
  startTargetActions(draft);
}

function finishTargetDetail(draft: Draft, requestId: number, result: Result<TargetDetail>) {
  if (draft.targets.detail.running !== requestId) return;
  draft.targets.detail.running = null;
  if (draft.targets.detail.wanted === requestId) {
    draft.targets.detail.wanted = null;
    acceptTargetDetail(draft, result);
  }
  startTargetDetail(draft);
}

function finishTargetOutputs(draft: Draft, requestId: number, result: Result<TargetOutputs>) {
  if (draft.targets.outputs.running !== requestId) return;
  draft.targets.outputs.running = null;
  if (draft.targets.outputs.wanted === requestId) acceptTargetOutputs(draft, result);
  resumeTargets(draft);
}

function finishTargetActions(draft: Draft, requestId: number, result: Result<TargetActions>) {
  if (draft.targets.actions.running !== requestId) return;
  draft.targets.actions.running = null;
  if (draft.targets.actions.wanted === requestId) acceptTargetActions(draft, result);
  resumeTargets(draft);
}

function acceptTargetOutputs(draft: Draft, result: Result<TargetOutputs>) {
  draft.targets.outputs.wanted = null;
  if (
    draft.targets.selection?.kind !== "target" ||
    draft.targets.detail.value?.target.label !== draft.targets.selection.label
  ) {
    return;
  }
  draft.targets.outputs.value = result.ok ? result.value : null;
  draft.targets.outputs.error = result.ok ? null : result.error;
}

function acceptTargetActions(draft: Draft, result: Result<TargetActions>) {
  draft.targets.actions.wanted = null;
  if (
    draft.targets.selection?.kind !== "target" ||
    draft.targets.detail.value?.target.label !== draft.targets.selection.label
  ) {
    return;
  }
  draft.targets.actions.value = result.ok ? result.value : null;
  draft.targets.actions.error = result.ok ? null : result.error;
}

function finishDetail(draft: Draft, requestId: number, result: Result<InvocationDetail>) {
  if (draft.saved.detail.running !== requestId) return;
  draft.saved.detail.running = null;
  if (draft.saved.detail.wanted !== requestId) {
    startDetail(draft);
    return;
  }
  draft.saved.detail.wanted = null;
  if (!savedEligible(draft)) return;
  if (result.ok) draft.saved.detail.value = result.value;
  draft.saved.detail.error = result.ok ? null : result.error;
}

function finishProfile(draft: Draft, requestId: number, result: Result<Section<Profile>>) {
  if (draft.saved.profile.running !== requestId) return;
  draft.saved.profile.running = null;
  if (draft.saved.profile.wanted !== requestId) {
    startProfile(draft);
    return;
  }
  draft.saved.profile.wanted = null;
  if (!savedEligible(draft)) return;
  if (result.ok) draft.saved.profile.value = result.value;
  draft.saved.profile.error = result.ok ? null : result.error;
}

function start(draft: Draft, cmd: BazelCommand, pattern: string, argumentsText: string) {
  if (!canSend(sessionView(draft), pattern) || !draft.connection.state?.workspace) return;
  const wasEligible = savedEligible(draft);
  const request: StartRequest = {
    id: nextId(draft),
    cmd,
    pattern,
    previousExecution: draft.execution.observed,
    observedId: null,
    recordId: null,
  };
  draft.execution.pendingStart = request;
  draft.connection.error = null;
  draft.effects.push({
    type: "startInvocation",
    id: request.id,
    cmd,
    pattern,
    argumentsText,
  });
  savedEligibilityChanged(draft, wasEligible, savedEligible(draft));
}

function startResult(draft: Draft, requestId: number, result: Result<InvocationRecord>) {
  const pending = draft.execution.pendingStart;
  if (!pending || pending.id !== requestId) return;
  if (!result.ok) {
    finishStart(draft, requestId);
    draft.connection.error = result.error;
    return;
  }
  draft.execution.pendingStart = { ...pending, recordId: result.value.id };
  if (draft.content.kind !== "console" || draft.execution.console?.id !== result.value.id) {
    draft.execution.console = { ...result.value, cmd: pending.cmd, pattern: pending.pattern };
    showContent(draft, { kind: "console", page: workspacePage(draft) });
  }
  draft.effects.push({
    type: "getStartState",
    startId: requestId,
    revision: draft.connection.revision,
  });
}

function startStateResult(
  draft: Draft,
  message: Extract<SessionMessage, { type: "startStateResult" }>,
) {
  if (draft.execution.pendingStart?.id !== message.startId) return;
  if (message.revision === draft.connection.revision) {
    if (message.result.ok) {
      applyAppState(draft, message.result.value);
    } else {
      const wasEligible = savedEligible(draft);
      draft.connection.ready = false;
      draft.connection.error = `The invocation started, but getting its state failed; waiting for the next update: ${message.result.error}`;
      savedEligibilityChanged(draft, wasEligible, savedEligible(draft));
    }
  }
  finishStart(draft, message.startId);
  if (draft.connection.ready) requestHistory(draft);
}

function initialStateResult(
  draft: Draft,
  message: Extract<SessionMessage, { type: "initialStateResult" }>,
) {
  if (message.revision !== draft.connection.revision) return;
  if (message.result.ok) applyAppState(draft, message.result.value);
  else if (!draft.connection.ready)
    draft.connection.error = `Could not connect: ${message.result.error}`;
}

function openConsole(draft: Draft, record: InvocationRecord) {
  const expected =
    draft.connection.state?.consoleInvocation ?? draft.connection.state?.activeInvocation;
  if (record.id !== expected) {
    selectSaved(draft, record.id);
    return;
  }
  if (draft.content.kind === "console" && draft.execution.console?.id === record.id) return;
  draft.execution.console = record;
  showContent(draft, { kind: "console", page: workspacePage(draft) });
}

function setWorkspacePromptOpen(draft: Draft, open: boolean) {
  if (draft.workspacePrompt.status === "opening") return;
  draft.workspacePrompt.status = open ? "editing" : "closed";
  draft.workspacePrompt.error = null;
}

function requestWorkspaceOpen(draft: Draft, path: string, scope: TargetScope) {
  if (sessionBusy(sessionView(draft))) {
    draft.workspacePrompt.error =
      "Wait for the current Bazel operation to finish before changing the workspace";
    return;
  }
  setWorkspacePromptOpen(draft, true);
  draft.workspacePrompt.status = "opening";
  draft.effects.push({ type: "openWorkspace", path, scope, revision: draft.connection.revision });
}

function finishWorkspaceOpen(draft: Draft, revision: number, result: Result<AppState>) {
  draft.workspacePrompt.status = result.ok ? "closed" : "editing";
  draft.workspacePrompt.error = result.ok ? null : result.error;
  if (result.ok && revision === draft.connection.revision) applyAppState(draft, result.value);
  resumeTargets(draft);
}

function requestStop(draft: Draft) {
  if (!canStop(sessionView(draft))) return;
  draft.effects.push({ type: "stop", workspace: draft.connection.state?.workspace ?? null });
}

function requestRefresh(draft: Draft) {
  if (sessionBusy(sessionView(draft))) return;
  draft.effects.push({ type: "refresh", workspace: draft.connection.state?.workspace ?? null });
}

export function update(model: SessionModel, message: SessionMessage): SessionTransition {
  const next = draft(model);
  switch (message.type) {
    case "subscriptionsFailed":
      next.connection.error = `Could not connect: ${message.error}`;
      break;
    case "initialStateResult":
      initialStateResult(next, message);
      break;
    case "appState":
      applyAppState(next, message.state);
      break;
    case "historyChanged":
    case "loadInvocations":
      requestHistory(next);
      break;
    case "invocationsResult":
      finishHistory(next, message.requestId, message.result);
      break;
    case "loadSnapshot":
      startSnapshot(next);
      break;
    case "snapshotResult":
      finishSnapshot(next, message.requestId, message.result);
      break;
    case "targetDetailResult":
      finishTargetDetail(next, message.requestId, message.result);
      break;
    case "targetOutputsResult":
      finishTargetOutputs(next, message.requestId, message.result);
      break;
    case "targetActionsResult":
      finishTargetActions(next, message.requestId, message.result);
      break;
    case "detailResult":
      finishDetail(next, message.requestId, message.result);
      break;
    case "profileResult":
      finishProfile(next, message.requestId, message.result);
      break;
    case "select":
      showPage(next, "targets");
      selectTarget(next, message.selection);
      break;
    case "selectInvocation":
      selectSaved(next, message.id);
      break;
    case "showPage":
      showPage(next, message.page);
      break;
    case "openConsole":
      openConsole(next, message.record);
      break;
    case "loadDetail":
      retryDetail(next);
      break;
    case "loadProfile":
      retryProfile(next);
      break;
    case "start":
      start(next, message.cmd, message.pattern, message.argumentsText);
      break;
    case "startResult":
      startResult(next, message.requestId, message.result);
      break;
    case "startStateResult":
      startStateResult(next, message);
      break;
    case "workspacePrompt":
      setWorkspacePromptOpen(next, message.open);
      break;
    case "openWorkspace":
      requestWorkspaceOpen(next, message.path, message.scope);
      break;
    case "workspaceResult":
      finishWorkspaceOpen(next, message.revision, message.result);
      break;
    case "stop":
      requestStop(next);
      break;
    case "stopResult":
      if (!message.result.ok && message.workspace === next.connection.state?.workspace) {
        next.connection.error = message.result.error;
      }
      break;
    case "refresh":
      requestRefresh(next);
      break;
    case "refreshResult":
      if (!message.result.ok && message.workspace === next.connection.state?.workspace) {
        next.connection.error = message.result.error;
      }
      break;
    default:
      unexpected(message);
  }
  return finishDraft(next);
}
