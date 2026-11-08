import { targetError } from "./target.ts";
import type {
  AppState,
  BazelCommand,
  ExplorerSelection,
  InvocationDetail,
  InvocationRecord,
  Profile,
  Section,
  TargetDetail,
  TargetIndex,
  TargetOutputs,
  TargetScope,
} from "./types.ts";

export interface SessionView {
  console: InvocationRecord | null;
  consoleOpen: boolean;
  state: AppState | null;
  snapshot: TargetIndex | null;
  snapshotLoading: boolean;
  snapshotError: string | null;
  selection: ExplorerSelection;
  targetDetail: TargetDetail | null;
  targetDetailError: string | null;
  targetOutputs: TargetOutputs | null;
  targetOutputsError: string | null;
  ready: boolean;
  starting: boolean;
  opening: boolean;
  workspacePromptOpen: boolean;
  workspaceError: string | null;
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

export type SessionData = Omit<
  SessionView,
  "snapshotLoading" | "starting" | "opening" | "detailLoading" | "profileLoading"
>;

interface Reader {
  running: number | null;
  wanted: number | null;
}

interface HistoryRequest {
  id: number;
  valid: boolean;
}

interface ObservedExecution {
  readonly id: string;
  readonly active: boolean;
}

interface StartRequest {
  readonly id: number;
  readonly workspace: string;
  readonly cmd: BazelCommand;
  readonly pattern: string;
  readonly previousExecution: ObservedExecution | null;
  readonly observedId: string | null;
  readonly record: InvocationRecord | null;
}

export interface SessionModel {
  readonly data: SessionData;
  readonly nextRequestId: number;
  readonly stateRevision: number;
  readonly pendingStart: StartRequest | null;
  readonly observedExecution: ObservedExecution | null;
  readonly opening: boolean;
  readonly historyRequest: HistoryRequest | null;
  readonly historyAgain: boolean;
  readonly snapshotRequestId: number | null;
  readonly targetDetail: Reader;
  readonly targetOutputs: Reader;
  readonly detail: Reader;
  readonly profile: Reader;
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
  | RequestResult<"detailResult", InvocationDetail>
  | RequestResult<"profileResult", Section<Profile>>
  | { readonly type: "select"; readonly selection: ExplorerSelection }
  | { readonly type: "selectInvocation"; readonly id: string | null }
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

function emptyData(): SessionData {
  return {
    console: null,
    consoleOpen: false,
    state: null,
    snapshot: null,
    snapshotError: null,
    selection: null,
    targetDetail: null,
    targetDetailError: null,
    targetOutputs: null,
    targetOutputsError: null,
    ready: false,
    workspacePromptOpen: false,
    workspaceError: null,
    error: null,
    invocations: [],
    invocationsError: null,
    selectedInvocation: null,
    detail: null,
    detailError: null,
    profile: null,
    profileError: null,
  };
}

function emptyReader(): Reader {
  return { running: null, wanted: null };
}

export function initialModel(): SessionModel {
  return {
    data: emptyData(),
    nextRequestId: 1,
    stateRevision: 0,
    pendingStart: null,
    observedExecution: null,
    opening: false,
    historyRequest: null,
    historyAgain: false,
    snapshotRequestId: null,
    targetDetail: emptyReader(),
    targetOutputs: emptyReader(),
    detail: emptyReader(),
    profile: emptyReader(),
  };
}

function savedEligible(model: SessionModel): boolean {
  const id = model.data.selectedInvocation;
  return !!(
    id &&
    model.data.ready &&
    model.data.state?.workspace &&
    model.pendingStart === null &&
    id !== model.data.state.activeInvocation
  );
}

export function sessionView(model: SessionModel): SessionView {
  const eligible = savedEligible(model);
  return {
    ...model.data,
    snapshotLoading: model.snapshotRequestId !== null,
    starting: model.pendingStart !== null,
    opening: model.opening,
    detailLoading:
      eligible && model.detail.running !== null && model.detail.running === model.detail.wanted,
    profileLoading:
      eligible && model.profile.running !== null && model.profile.running === model.profile.wanted,
  };
}

export function emptyView(): SessionView {
  return sessionView(initialModel());
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
    data: { ...model.data },
    historyRequest: model.historyRequest ? { ...model.historyRequest } : null,
    targetDetail: { ...model.targetDetail },
    targetOutputs: { ...model.targetOutputs },
    detail: { ...model.detail },
    profile: { ...model.profile },
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

function setData(draft: Draft, patch: Partial<SessionData>) {
  Object.assign(draft.data, patch);
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
  if (!draft.data.ready || !draft.data.state?.workspace || draft.historyRequest) {
    return;
  }
  const id = nextId(draft);
  draft.historyRequest = { id, valid: true };
  draft.historyAgain = false;
  draft.data.invocationsError = null;
  draft.effects.push({ type: "listInvocations", id });
}

function requestHistory(draft: Draft) {
  if (draft.historyRequest) {
    draft.historyAgain = true;
  } else {
    startHistory(draft);
  }
}

function startSnapshot(draft: Draft) {
  const state = draft.data.state;
  if (!state?.workspace || state.snapshotFetchedAt === null) return;
  if (draft.snapshotRequestId !== null) return;
  const id = nextId(draft);
  draft.snapshotRequestId = id;
  draft.data.snapshotError = null;
  draft.effects.push({ type: "getSnapshot", id });
}

function targetReadArgs(draft: Draft): { label: string; fetchedAt: number } | null {
  const selection = draft.data.selection;
  const state = draft.data.state;
  const snapshot = draft.data.snapshot;
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
  if (draft.targetDetail.running !== null || draft.targetDetail.wanted === null) return;
  const args = targetReadArgs(draft);
  if (!args) return;
  const id = draft.targetDetail.wanted;
  draft.targetDetail.running = id;
  draft.effects.push({ type: "getTargetDetail", id, ...args });
}

function startTargetOutputs(draft: Draft) {
  if (draft.targetOutputs.running !== null || draft.targetOutputs.wanted === null) return;
  const args = targetReadArgs(draft);
  if (!args || draft.data.targetDetail?.target.label !== args.label) return;
  const id = draft.targetOutputs.wanted;
  draft.targetOutputs.running = id;
  draft.effects.push({ type: "getTargetOutputs", id, ...args });
}

function startDetail(draft: Draft) {
  if (draft.detail.running !== null || draft.detail.wanted === null || !savedEligible(draft))
    return;
  const invocationId = draft.data.selectedInvocation;
  if (!invocationId) return;
  const id = draft.detail.wanted;
  draft.detail.running = id;
  draft.data.detailError = null;
  draft.effects.push({ type: "getInvocation", id, invocationId });
}

function startProfile(draft: Draft) {
  if (draft.profile.running !== null || draft.profile.wanted === null || !savedEligible(draft))
    return;
  const invocationId = draft.data.selectedInvocation;
  if (!invocationId) return;
  const id = draft.profile.wanted;
  draft.profile.running = id;
  draft.data.profileError = null;
  draft.effects.push({ type: "getProfile", id, invocationId });
}

function resumeTargets(draft: Draft) {
  startTargetDetail(draft);
  startTargetOutputs(draft);
}

function resumeSaved(draft: Draft) {
  startDetail(draft);
  startProfile(draft);
}

function retryDetail(draft: Draft) {
  if (draft.data.selectedInvocation === null) return;
  if (draft.detail.wanted === null) draft.detail.wanted = nextId(draft);
  startDetail(draft);
}

function retryProfile(draft: Draft) {
  if (draft.data.selectedInvocation === null) return;
  if (draft.profile.wanted === null) draft.profile.wanted = nextId(draft);
  startProfile(draft);
}

function selectSaved(draft: Draft, id: string | null, retrySame = false) {
  if (draft.data.selectedInvocation === id) {
    if (retrySame) {
      retryDetail(draft);
      retryProfile(draft);
    }
    return;
  }
  draft.data.selectedInvocation = id;
  setData(draft, {
    detail: null,
    detailError: null,
    profile: null,
    profileError: null,
  });
  draft.detail.wanted = id === null ? null : nextId(draft);
  draft.profile.wanted = id === null ? null : nextId(draft);
  resumeSaved(draft);
}

function savedEligibilityChanged(draft: Draft, wasEligible: boolean, isEligible: boolean) {
  if (wasEligible === isEligible) return;
  if (isEligible) {
    resumeSaved(draft);
    return;
  }
  if (draft.data.selectedInvocation === null) {
    draft.detail.wanted = null;
    draft.profile.wanted = null;
    return;
  }
  revokeReaderIntent(draft, draft.detail);
  revokeReaderIntent(draft, draft.profile);
}

function revokeReaderIntent(draft: Draft, reader: Reader) {
  if (reader.running !== null && reader.wanted === reader.running) {
    reader.wanted = nextId(draft);
  }
}

function refreshSavedFromHistory(draft: Draft) {
  if (draft.data.selectedInvocation === null) return;
  draft.detail.wanted = nextId(draft);
  draft.profile.wanted = nextId(draft);
  draft.data.detailError = null;
  draft.data.profileError = null;
  resumeSaved(draft);
}

function queueTargetDetail(draft: Draft) {
  if (draft.data.selection?.kind !== "target") {
    draft.targetDetail.wanted = null;
    draft.targetOutputs.wanted = null;
    return;
  }
  draft.targetDetail.wanted = nextId(draft);
  draft.targetOutputs.wanted = null;
}

function resetTargetContext(draft: Draft) {
  setData(draft, {
    targetDetail: null,
    targetDetailError: null,
    targetOutputs: null,
    targetOutputsError: null,
  });
  queueTargetDetail(draft);
  resumeTargets(draft);
}

function selectTarget(draft: Draft, selection: ExplorerSelection) {
  if (sameSelection(draft.data.selection, selection)) {
    if (selection?.kind === "target" && draft.data.targetDetailError !== null) {
      draft.data.targetDetailError = null;
      queueTargetDetail(draft);
      startTargetDetail(draft);
    }
    return;
  }
  draft.data.selection = selection;
  resetTargetContext(draft);
}

function resetWorkspaceData(draft: Draft, state: AppState) {
  draft.pendingStart = null;
  draft.observedExecution = state.activeInvocation
    ? { id: state.activeInvocation, active: true }
    : null;
  draft.snapshotRequestId = null;
  if (draft.historyRequest) draft.historyRequest.valid = false;
  draft.historyAgain = true;
  selectSaved(draft, null);
  draft.data.selection = null;
  resetTargetContext(draft);
  setData(draft, {
    console: null,
    consoleOpen: false,
    snapshot: null,
    snapshotError: null,
    invocations: [],
    invocationsError: null,
  });
}

function applyPendingStartState(draft: Draft, state: AppState): boolean {
  const starting = draft.pendingStart;
  const activeId = state.activeInvocation;
  if (
    !starting ||
    starting.workspace !== state.workspace ||
    !activeId ||
    state.consoleInvocation !== activeId ||
    starting.observedId === activeId
  ) {
    return false;
  }
  draft.pendingStart = { ...starting, observedId: activeId };
  setData(draft, {
    console: { id: activeId, cmd: starting.cmd, pattern: starting.pattern },
    consoleOpen: true,
  });
  selectSaved(draft, null);
  return true;
}

function observedCompletionId(
  draft: Draft,
  before: AppState | null,
  state: AppState,
  activeId: string | null,
): string | null {
  const observed = draft.observedExecution;
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
  draft.observedExecution = activeId ? { id: activeId, active: true } : null;
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
    selectionCleared = draft.data.selection !== null;
    draft.data.selection = null;
    setData(draft, { snapshot: null, snapshotError: null });
  }
  if ((contextChanged && !workspaceChanged) || selectionCleared) resetTargetContext(draft);
  if (!contextChanged) {
    if (draft.data.snapshotError !== null) startSnapshot(draft);
    return;
  }
  draft.snapshotRequestId = null;
  if (state.snapshotFetchedAt === null || scopeChanged || workspaceChanged) {
    draft.data.snapshot = null;
  }
  startSnapshot(draft);
}

function applyAppState(draft: Draft, state: AppState) {
  const before = draft.data.state;
  const wasReady = draft.data.ready;
  const wasSavedEligible = savedEligible(draft);
  const wasBusy = sessionBusy(sessionView(draft));
  draft.stateRevision++;
  draft.data.state = state;
  draft.data.ready = true;
  if (!wasReady) draft.data.error = null;
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
    draft.pendingStart ||
    draft.data.state?.activeInvocation ||
    !draft.data.consoleOpen ||
    draft.data.console?.id !== id
  ) {
    return;
  }
  draft.data.consoleOpen = false;
  selectSaved(draft, id);
}

function finishStart(draft: Draft, requestId: number) {
  const current = draft.pendingStart;
  if (!current || current.id !== requestId) return;
  const wasEligible = savedEligible(draft);
  draft.pendingStart = null;
  if (!current.record) {
    draft.observedExecution = current.previousExecution;
  } else {
    const id = current.record.id;
    if (!draft.data.ready) {
      draft.observedExecution = { id, active: current.observedId === id };
    } else if (draft.data.state?.activeInvocation === id) {
      draft.observedExecution = { id, active: true };
    } else if (!draft.data.state?.activeInvocation) {
      draft.observedExecution = null;
      completeConsole(draft, id);
    } else {
      draft.observedExecution = { id: draft.data.state.activeInvocation, active: true };
    }
  }
  savedEligibilityChanged(draft, wasEligible, savedEligible(draft));
  resumeTargets(draft);
}

function acceptHistory(draft: Draft, records: InvocationRecord[]) {
  const id = draft.data.selectedInvocation;
  const previous = recordFor(draft.data.invocations, id);
  const next = recordFor(records, id);
  const changed = JSON.stringify(previous ?? null) !== JSON.stringify(next ?? null);
  draft.data.invocations = records;
  draft.data.invocationsError = null;
  const detailMismatch =
    next !== undefined &&
    draft.data.detail !== null &&
    draft.detail.running === null &&
    draft.data.detailError === null &&
    JSON.stringify(next) !== JSON.stringify(draft.data.detail.record);
  if (changed || detailMismatch) refreshSavedFromHistory(draft);
}

function finishHistory(draft: Draft, requestId: number, result: Result<InvocationRecord[]>) {
  const current = draft.historyRequest;
  if (!current || current.id !== requestId) return;
  draft.historyRequest = null;
  if (current.valid) {
    if (result.ok) acceptHistory(draft, result.value);
    else draft.data.invocationsError = result.error;
  }
  if (draft.historyAgain) {
    draft.historyAgain = false;
    startHistory(draft);
  }
}

function acceptSnapshot(draft: Draft, snapshot: TargetIndex | null) {
  const previousFetchedAt = draft.data.snapshot?.meta.fetchedAt ?? null;
  const selection = draft.data.selection;
  const validTarget =
    selection?.kind === "target" &&
    snapshot?.targets.some((target) => target.label === selection.label);
  const selectionInvalid =
    selection?.kind === "package" || (selection?.kind === "target" && !validTarget);
  draft.data.snapshot = snapshot;
  draft.data.snapshotError = null;
  if (draft.data.targetDetailError === TARGET_LIST_ERROR) {
    draft.data.targetDetailError = null;
  }
  if (selectionInvalid) draft.data.selection = null;
  if (previousFetchedAt !== (snapshot?.meta.fetchedAt ?? null) || selectionInvalid) {
    resetTargetContext(draft);
  }
  resumeTargets(draft);
}

function failSnapshot(draft: Draft, error: string) {
  draft.data.snapshotError = `Could not load the workspace snapshot: ${error}`;
  if (draft.data.selection?.kind === "target") draft.data.targetDetailError = TARGET_LIST_ERROR;
}

function finishSnapshot(draft: Draft, requestId: number, result: Result<TargetIndex | null>) {
  if (draft.snapshotRequestId !== requestId) return;
  draft.snapshotRequestId = null;
  if (result.ok) acceptSnapshot(draft, result.value);
  else failSnapshot(draft, result.error);
}

function acceptTargetDetail(draft: Draft, result: Result<TargetDetail>) {
  const selection = draft.data.selection;
  if (!result.ok) {
    draft.data.targetDetailError = result.error;
    return;
  }
  if (selection?.kind !== "target" || result.value.target.label !== selection.label) {
    draft.data.targetDetailError = "Error: Target detail does not match the selected label";
    return;
  }
  setData(draft, { targetDetail: result.value, targetDetailError: null });
  if (
    draft.data.targetOutputs === null &&
    draft.data.targetOutputsError === null &&
    draft.targetOutputs.wanted === null
  ) {
    draft.targetOutputs.wanted = nextId(draft);
  }
  startTargetOutputs(draft);
}

function finishTargetDetail(draft: Draft, requestId: number, result: Result<TargetDetail>) {
  if (draft.targetDetail.running !== requestId) return;
  draft.targetDetail.running = null;
  if (draft.targetDetail.wanted === requestId) {
    draft.targetDetail.wanted = null;
    acceptTargetDetail(draft, result);
  }
  startTargetDetail(draft);
}

function finishTargetOutputs(draft: Draft, requestId: number, result: Result<TargetOutputs>) {
  if (draft.targetOutputs.running !== requestId) return;
  draft.targetOutputs.running = null;
  if (draft.targetOutputs.wanted === requestId) acceptTargetOutputs(draft, result);
  startTargetOutputs(draft);
}

function acceptTargetOutputs(draft: Draft, result: Result<TargetOutputs>) {
  draft.targetOutputs.wanted = null;
  if (
    draft.data.selection?.kind !== "target" ||
    draft.data.targetDetail?.target.label !== draft.data.selection.label
  ) {
    return;
  }
  setData(draft, {
    targetOutputs: result.ok ? result.value : null,
    targetOutputsError: result.ok ? null : result.error,
  });
}

function finishDetail(draft: Draft, requestId: number, result: Result<InvocationDetail>) {
  if (draft.detail.running !== requestId) return;
  draft.detail.running = null;
  if (draft.detail.wanted !== requestId) {
    startDetail(draft);
    return;
  }
  draft.detail.wanted = null;
  if (!savedEligible(draft)) return;
  if (result.ok) draft.data.detail = result.value;
  draft.data.detailError = result.ok ? null : result.error;
}

function finishProfile(draft: Draft, requestId: number, result: Result<Section<Profile>>) {
  if (draft.profile.running !== requestId) return;
  draft.profile.running = null;
  if (draft.profile.wanted !== requestId) {
    startProfile(draft);
    return;
  }
  draft.profile.wanted = null;
  if (!savedEligible(draft)) return;
  if (result.ok) draft.data.profile = result.value;
  draft.data.profileError = result.ok ? null : result.error;
}

function start(draft: Draft, cmd: BazelCommand, pattern: string, argumentsText: string) {
  if (!canSend(sessionView(draft), pattern) || !draft.data.state?.workspace) return;
  const wasEligible = savedEligible(draft);
  const request: StartRequest = {
    id: nextId(draft),
    workspace: draft.data.state.workspace,
    cmd,
    pattern,
    previousExecution: draft.observedExecution,
    observedId: null,
    record: null,
  };
  draft.pendingStart = request;
  draft.data.error = null;
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
  const pending = draft.pendingStart;
  if (!pending || pending.id !== requestId) return;
  if (!result.ok) {
    finishStart(draft, requestId);
    draft.data.error = result.error;
    return;
  }
  draft.pendingStart = { ...pending, record: result.value };
  if (!draft.data.consoleOpen || draft.data.console?.id !== result.value.id) {
    setData(draft, {
      console: { ...result.value, cmd: pending.cmd, pattern: pending.pattern },
      consoleOpen: true,
    });
    selectSaved(draft, null);
  }
  draft.effects.push({
    type: "getStartState",
    startId: requestId,
    revision: draft.stateRevision,
  });
}

function startStateResult(
  draft: Draft,
  message: Extract<SessionMessage, { type: "startStateResult" }>,
) {
  if (draft.pendingStart?.id !== message.startId) return;
  if (message.revision === draft.stateRevision) {
    if (message.result.ok) {
      applyAppState(draft, message.result.value);
    } else {
      const wasEligible = savedEligible(draft);
      setData(draft, {
        ready: false,
        error: `The invocation started, but getting its state failed; waiting for the next update: ${message.result.error}`,
      });
      savedEligibilityChanged(draft, wasEligible, savedEligible(draft));
    }
  }
  finishStart(draft, message.startId);
  if (draft.data.ready) requestHistory(draft);
}

function initialStateResult(
  draft: Draft,
  message: Extract<SessionMessage, { type: "initialStateResult" }>,
) {
  if (message.revision !== draft.stateRevision) return;
  if (message.result.ok) applyAppState(draft, message.result.value);
  else if (!draft.data.ready) draft.data.error = `Could not connect: ${message.result.error}`;
}

function openConsole(draft: Draft, record: InvocationRecord) {
  const expected = draft.data.state?.consoleInvocation ?? draft.data.state?.activeInvocation;
  if (record.id !== expected) {
    draft.data.consoleOpen = false;
    selectSaved(draft, record.id, true);
    return;
  }
  if (draft.data.consoleOpen && draft.data.console?.id === record.id) return;
  selectSaved(draft, null);
  setData(draft, { console: record, consoleOpen: true });
}

function setWorkspacePromptOpen(draft: Draft, open: boolean) {
  if (draft.opening) return;
  setData(draft, { workspacePromptOpen: open, workspaceError: null });
}

function requestWorkspaceOpen(draft: Draft, path: string, scope: TargetScope) {
  if (sessionBusy(sessionView(draft))) {
    draft.data.workspaceError =
      "Wait for the current Bazel operation to finish before changing the workspace";
    return;
  }
  setWorkspacePromptOpen(draft, true);
  draft.opening = true;
  draft.effects.push({ type: "openWorkspace", path, scope, revision: draft.stateRevision });
}

function finishWorkspaceOpen(draft: Draft, revision: number, result: Result<AppState>) {
  draft.opening = false;
  setData(draft, {
    workspacePromptOpen: !result.ok,
    workspaceError: result.ok ? null : result.error,
  });
  if (result.ok && revision === draft.stateRevision) applyAppState(draft, result.value);
  resumeTargets(draft);
}

function requestStop(draft: Draft) {
  if (!canStop(sessionView(draft))) return;
  draft.effects.push({ type: "stop", workspace: draft.data.state?.workspace ?? null });
}

function requestRefresh(draft: Draft) {
  if (sessionBusy(sessionView(draft))) return;
  draft.effects.push({ type: "refresh", workspace: draft.data.state?.workspace ?? null });
}

export function update(model: SessionModel, message: SessionMessage): SessionTransition {
  const next = draft(model);
  switch (message.type) {
    case "subscriptionsFailed":
      next.data.error = `Could not connect: ${message.error}`;
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
    case "detailResult":
      finishDetail(next, message.requestId, message.result);
      break;
    case "profileResult":
      finishProfile(next, message.requestId, message.result);
      break;
    case "select":
      selectTarget(next, message.selection);
      break;
    case "selectInvocation":
      next.data.consoleOpen = false;
      selectSaved(next, message.id, true);
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
      if (!message.result.ok && message.workspace === next.data.state?.workspace) {
        next.data.error = message.result.error;
      }
      break;
    case "refresh":
      requestRefresh(next);
      break;
    case "refreshResult":
      if (!message.result.ok && message.workspace === next.data.state?.workspace) {
        next.data.error = message.result.error;
      }
      break;
    default:
      unexpected(message);
  }
  return finishDraft(next);
}
