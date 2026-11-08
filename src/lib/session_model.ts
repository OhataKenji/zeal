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

interface HistoryRequest {
  readonly id: number;
  readonly workspaceGeneration: number;
}

interface SnapshotRequest {
  readonly id: number;
  readonly key: string;
}

interface TargetRequest {
  readonly id: number;
  readonly key: string;
  readonly label: string;
  readonly fetchedAt: number;
}

interface SavedRequest {
  readonly id: number;
  readonly key: string;
  readonly invocationId: string;
}

interface WorkspaceRequest {
  readonly id: number;
  readonly path: string;
  readonly scope: TargetScope;
  readonly revision: number;
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
  readonly argumentsText: string;
  readonly previousExecution: ObservedExecution | null;
  readonly observedId: string | null;
  readonly record: InvocationRecord | null;
}

export interface SessionModel {
  readonly data: SessionData;
  readonly nextRequestId: number;
  readonly stateRevision: number;
  readonly workspaceGeneration: number;
  readonly invocationGeneration: number;
  readonly selectionGeneration: number;
  readonly pendingStart: StartRequest | null;
  readonly observedExecution: ObservedExecution | null;
  readonly workspaceRequest: WorkspaceRequest | null;
  readonly historyRequest: HistoryRequest | null;
  readonly historyAttempt: string | null;
  readonly historyAgain: boolean;
  readonly snapshotRequest: SnapshotRequest | null;
  readonly snapshotAttempt: string | null;
  readonly targetDetailRequest: TargetRequest | null;
  readonly targetDetailAttempt: string | null;
  readonly targetOutputsRequest: TargetRequest | null;
  readonly detailRequest: SavedRequest | null;
  readonly detailAttempt: string | null;
  readonly profileRequest: SavedRequest | null;
  readonly profileAttempt: string | null;
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
  | { readonly type: "listInvocations"; readonly request: HistoryRequest }
  | { readonly type: "getSnapshot"; readonly request: SnapshotRequest }
  | { readonly type: "getTargetDetail"; readonly request: TargetRequest }
  | { readonly type: "getTargetOutputs"; readonly request: TargetRequest }
  | { readonly type: "getInvocation"; readonly request: SavedRequest }
  | { readonly type: "getProfile"; readonly request: SavedRequest }
  | { readonly type: "startInvocation"; readonly request: StartRequest }
  | {
      readonly type: "getStartState";
      readonly id: number;
      readonly startId: number;
      readonly revision: number;
    }
  | { readonly type: "openWorkspace"; readonly request: WorkspaceRequest }
  | { readonly type: "rejectWorkspaceOpen"; readonly id: number; readonly message: string }
  | { readonly type: "stop"; readonly id: number; readonly workspace: string | null }
  | { readonly type: "refresh"; readonly id: number; readonly workspace: string | null };

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
  | { readonly type: "openWorkspace"; readonly path: string; readonly scope: TargetScope }
  | RequestResult<"workspaceResult", AppState>
  | { readonly type: "stop" }
  | (ResultMessage<"stopResult", void> & { readonly workspace: string | null })
  | { readonly type: "refresh" }
  | (ResultMessage<"refreshResult", void> & { readonly workspace: string | null });

interface SessionTransition {
  readonly model: SessionModel;
  readonly effects: SessionEffect[];
  readonly waitFor?: number;
}

const RUNNING_SLOTS = ["ingest", "detailQuery"] as const;
const TARGET_LIST_ERROR = "Could not load the target list";
const OPEN_WORKSPACE_ERROR =
  "Wait for the current Bazel operation to finish before changing the workspace";

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

export function initialModel(): SessionModel {
  return {
    data: emptyData(),
    nextRequestId: 1,
    stateRevision: 0,
    workspaceGeneration: 0,
    invocationGeneration: 0,
    selectionGeneration: 0,
    pendingStart: null,
    observedExecution: null,
    workspaceRequest: null,
    historyRequest: null,
    historyAttempt: null,
    historyAgain: false,
    snapshotRequest: null,
    snapshotAttempt: null,
    targetDetailRequest: null,
    targetDetailAttempt: null,
    targetOutputsRequest: null,
    detailRequest: null,
    detailAttempt: null,
    profileRequest: null,
    profileAttempt: null,
  };
}

export function sessionView(model: SessionModel): SessionView {
  const key = savedKey(model);
  const eligible = savedEligible(model);
  return {
    ...model.data,
    snapshotLoading: model.snapshotRequest?.key === snapshotKey(model),
    starting: model.pendingStart !== null,
    opening: model.workspaceRequest !== null,
    detailLoading: eligible && model.detailRequest?.key === key,
    profileLoading: eligible && model.profileRequest?.key === key,
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
  waitFor?: number;
};

function draft(model: SessionModel): Draft {
  // Only these fresh containers are mutable; nested requests and payloads stay shared.
  return { ...model, data: { ...model.data }, effects: [] };
}

function finishDraft(draft: Draft): SessionTransition {
  const { effects, waitFor, ...model } = draft;
  return { model, effects, waitFor };
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

function scopeKey(state: AppState | null): string {
  return JSON.stringify(state?.scope.include ?? []);
}

function sameSelection(left: ExplorerSelection, right: ExplorerSelection): boolean {
  if (left?.kind !== right?.kind) return false;
  if (left?.kind === "target" && right?.kind === "target") return left.label === right.label;
  if (left?.kind === "package" && right?.kind === "package") return left.path === right.path;
  return left === null && right === null;
}

function setSelection(draft: Draft, selection: ExplorerSelection) {
  if (sameSelection(draft.data.selection, selection)) return;
  draft.selectionGeneration++;
  draft.data.selection = selection;
}

function setSelectedInvocation(draft: Draft, id: string | null) {
  if (draft.data.selectedInvocation === id) return;
  draft.invocationGeneration++;
  Object.assign(draft.data, {
    selectedInvocation: id,
    detail: null,
    detailError: null,
    profile: null,
    profileError: null,
  });
}

function selectedRecord(model: SessionModel): InvocationRecord | undefined {
  return model.data.invocations.find((record) => record.id === model.data.selectedInvocation);
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

function savedKey(model: SessionModel): string {
  const id = model.data.selectedInvocation;
  return JSON.stringify([
    model.data.state?.workspace ?? null,
    model.workspaceGeneration,
    model.invocationGeneration,
    id,
    savedEligible(model),
    JSON.stringify(selectedRecord(model) ?? null),
  ]);
}

function snapshotKey(model: SessionModel): string {
  return JSON.stringify([
    model.data.state?.workspace ?? null,
    model.workspaceGeneration,
    scopeKey(model.data.state),
    model.data.state?.snapshotFetchedAt ?? null,
  ]);
}

function targetKey(model: SessionModel): string {
  const selection = model.data.selection;
  const target =
    selection?.kind === "target"
      ? ["target", selection.label]
      : selection?.kind === "package"
        ? ["package", selection.path]
        : null;
  return JSON.stringify([
    model.data.state?.workspace ?? null,
    model.workspaceGeneration,
    scopeKey(model.data.state),
    target,
    model.selectionGeneration,
    model.data.state?.snapshotFetchedAt ?? null,
    model.data.snapshot?.meta.fetchedAt ?? null,
  ]);
}

function historyKey(model: SessionModel): string {
  return JSON.stringify([
    model.data.state?.workspace ?? null,
    model.workspaceGeneration,
    model.data.state?.activeInvocation ?? null,
    model.data.ready,
  ]);
}

function targetReady(model: SessionModel): boolean {
  const selection = model.data.selection;
  const state = model.data.state;
  const snapshot = model.data.snapshot;
  const view = sessionView(model);
  return !!(
    selection?.kind === "target" &&
    snapshot &&
    snapshot.targets.some((target) => target.label === selection.label) &&
    snapshot.meta.fetchedAt === state?.snapshotFetchedAt &&
    workspaceReady(view) &&
    !sessionBusy(view)
  );
}

function resetTargetContext(draft: Draft, previousKey: string) {
  if (previousKey === targetKey(draft)) return;
  draft.targetDetailAttempt = null;
  setData(draft, {
    targetDetail: null,
    targetDetailError: null,
    targetOutputs: null,
    targetOutputsError: null,
  });
}

function syncHistory(draft: Draft): number | undefined {
  const key = historyKey(draft);
  if (draft.historyAttempt !== key) draft.historyAttempt = null;
  if (!draft.data.state?.workspace || !draft.data.ready || draft.historyRequest) {
    return draft.historyRequest?.id;
  }
  if (draft.historyAttempt === key) return;
  const request = { id: nextId(draft), workspaceGeneration: draft.workspaceGeneration };
  draft.historyRequest = request;
  draft.historyAttempt = key;
  draft.data.invocationsError = null;
  draft.effects.push({ type: "listInvocations", request });
  return request.id;
}

function syncSnapshot(draft: Draft, authoritative = false) {
  const state = draft.data.state;
  const key = snapshotKey(draft);
  if (draft.snapshotAttempt !== key) {
    draft.snapshotAttempt = null;
    draft.data.snapshotError = null;
  }
  if (!state?.workspace || state.snapshotFetchedAt === null) return;
  if (authoritative && draft.data.snapshotError !== null) draft.snapshotAttempt = null;
  if (draft.snapshotRequest?.key === key || draft.snapshotAttempt === key) return;
  const request = { id: nextId(draft), key };
  draft.snapshotRequest = request;
  draft.snapshotAttempt = key;
  draft.data.snapshotError = null;
  draft.effects.push({ type: "getSnapshot", request });
}

function startTargetDetail(draft: Draft) {
  if (!targetReady(draft) || draft.targetDetailRequest) return;
  const selection = draft.data.selection;
  const fetchedAt = draft.data.state?.snapshotFetchedAt;
  if (selection?.kind !== "target" || fetchedAt == null) return;
  const key = targetKey(draft);
  if (draft.targetDetailAttempt === key) return;
  const request = { id: nextId(draft), key, label: selection.label, fetchedAt };
  draft.targetDetailRequest = request;
  draft.targetDetailAttempt = key;
  draft.data.targetDetailError = null;
  draft.effects.push({ type: "getTargetDetail", request });
}

function startTargetOutputs(draft: Draft) {
  const selection = draft.data.selection;
  const fetchedAt = draft.data.state?.snapshotFetchedAt;
  if (
    !targetReady(draft) ||
    selection?.kind !== "target" ||
    draft.data.targetDetail?.target.label !== selection.label ||
    draft.targetOutputsRequest ||
    draft.data.targetOutputs ||
    draft.data.targetOutputsError !== null
  ) {
    return;
  }
  if (fetchedAt == null) return;
  const request = { id: nextId(draft), key: targetKey(draft), label: selection.label, fetchedAt };
  draft.targetOutputsRequest = request;
  draft.data.targetOutputsError = null;
  draft.effects.push({ type: "getTargetOutputs", request });
}

function startDetail(draft: Draft): number | undefined {
  const key = savedKey(draft);
  if (!savedEligible(draft)) return;
  if (draft.detailRequest) return draft.detailRequest.id;
  if (draft.detailAttempt === key) return;
  const invocationId = draft.data.selectedInvocation;
  if (!invocationId) return;
  const request = { id: nextId(draft), key, invocationId };
  draft.detailRequest = request;
  draft.detailAttempt = key;
  draft.data.detailError = null;
  draft.effects.push({ type: "getInvocation", request });
  return request.id;
}

function startProfile(draft: Draft): number | undefined {
  const key = savedKey(draft);
  if (!savedEligible(draft)) return;
  if (draft.profileRequest) return draft.profileRequest.id;
  if (draft.profileAttempt === key) return;
  const invocationId = draft.data.selectedInvocation;
  if (!invocationId) return;
  const request = { id: nextId(draft), key, invocationId };
  draft.profileRequest = request;
  draft.profileAttempt = key;
  draft.data.profileError = null;
  draft.effects.push({ type: "getProfile", request });
  return request.id;
}

function syncAll(draft: Draft, authoritative = false) {
  syncHistory(draft);
  syncSnapshot(draft, authoritative);
  startTargetDetail(draft);
  startTargetOutputs(draft);
  syncSaved(draft);
}

function syncSaved(draft: Draft) {
  const key = savedKey(draft);
  if (draft.detailAttempt !== key) {
    draft.detailAttempt = null;
    draft.data.detailError = null;
  }
  if (draft.profileAttempt !== key) {
    draft.profileAttempt = null;
    draft.data.profileError = null;
  }
  startDetail(draft);
  startProfile(draft);
}

function refreshSavedAfterHistory(draft: Draft) {
  const record = selectedRecord(draft);
  if (
    record &&
    draft.data.detail &&
    !sessionView(draft).detailLoading &&
    draft.data.detailError === null &&
    JSON.stringify(record) !== JSON.stringify(draft.data.detail.record)
  ) {
    draft.detailAttempt = null;
    draft.profileAttempt = null;
    draft.data.profileError = null;
  }
}

function applyAppState(draft: Draft, state: AppState) {
  const before = draft.data.state;
  const workspaceChanged = state.workspace !== before?.workspace;
  const scopeChanged =
    JSON.stringify(state.scope.include) !== JSON.stringify(before?.scope.include);
  const oldTargetKey = targetKey(draft);
  const resumed = !draft.data.ready;
  draft.stateRevision++;
  draft.data.state = state;
  draft.data.ready = true;
  if (resumed) draft.data.error = null;

  if (workspaceChanged) {
    draft.workspaceGeneration++;
    draft.pendingStart = null;
    draft.historyAgain = false;
    draft.observedExecution = state.activeInvocation
      ? { id: state.activeInvocation, active: true }
      : null;
    draft.snapshotAttempt = null;
    draft.snapshotRequest = null;
    setData(draft, {
      console: null,
      consoleOpen: false,
      snapshot: null,
      snapshotError: null,
      selection: null,
      targetDetail: null,
      targetDetailError: null,
      targetOutputs: null,
      targetOutputsError: null,
      invocations: [],
      invocationsError: null,
      detail: null,
      detailError: null,
      profile: null,
      profileError: null,
    });
    setSelectedInvocation(draft, null);
    setSelection(draft, null);
  } else {
    if (scopeChanged || state.snapshotFetchedAt === null) {
      setSelection(draft, null);
      draft.snapshotAttempt = null;
      draft.snapshotRequest = null;
      setData(draft, { snapshot: null, snapshotError: null });
    }

    const starting = draft.pendingStart;
    if (starting) {
      if (
        starting.workspace === state.workspace &&
        state.activeInvocation &&
        state.consoleInvocation === state.activeInvocation &&
        starting.observedId !== state.activeInvocation
      ) {
        draft.pendingStart = { ...starting, observedId: state.activeInvocation };
        setData(draft, {
          console: { id: state.activeInvocation, cmd: starting.cmd, pattern: starting.pattern },
          consoleOpen: true,
        });
        setSelectedInvocation(draft, null);
      }
    } else {
      const observed = draft.observedExecution;
      const activeId = state.activeInvocation;
      let completedId: string | null = null;
      if (activeId) {
        if (observed && observed.active && before?.activeInvocation === observed.id) {
          if (observed.id !== activeId) completedId = observed.id;
        }
        draft.observedExecution = { id: activeId, active: true };
      } else {
        if (
          observed &&
          ((observed.active && before?.activeInvocation === observed.id) ||
            state.consoleInvocation === observed.id)
        ) {
          completedId = observed.id;
        }
        draft.observedExecution = null;
      }
      if (completedId) completeConsole(draft, completedId);
    }
  }

  resetTargetContext(draft, oldTargetKey);
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
  setSelectedInvocation(draft, id);
}

function finishStart(draft: Draft, requestId: number) {
  const current = draft.pendingStart;
  if (!current || current.id !== requestId) return;
  if (!current.record) {
    draft.pendingStart = null;
    draft.observedExecution = current.previousExecution;
    return;
  }
  const id = current.record.id;
  draft.pendingStart = null;
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

function loadHistory(draft: Draft) {
  if (!draft.data.state?.workspace || !draft.data.ready) return;
  if (draft.historyRequest) {
    draft.historyAgain = true;
    draft.waitFor = draft.historyRequest.id;
    return;
  }
  draft.historyAttempt = null;
  draft.waitFor = syncHistory(draft);
}

function loadSnapshot(draft: Draft) {
  const key = snapshotKey(draft);
  if (draft.snapshotRequest?.key === key) {
    draft.waitFor = draft.snapshotRequest.id;
    return;
  }
  if (!draft.data.state?.workspace || draft.data.state.snapshotFetchedAt === null) return;
  draft.snapshotAttempt = null;
  syncSnapshot(draft);
  draft.waitFor = draft.snapshotRequest?.id;
}

function forceDetail(draft: Draft) {
  if (!savedEligible(draft)) return;
  const key = savedKey(draft);
  if (draft.detailRequest?.key === key) {
    draft.waitFor = draft.detailRequest.id;
    return;
  }
  if (draft.detailRequest) {
    draft.detailAttempt = null;
    draft.waitFor = draft.detailRequest.id;
    return;
  }
  draft.detailAttempt = null;
  draft.waitFor = startDetail(draft);
}

function forceProfile(draft: Draft) {
  if (!savedEligible(draft)) return;
  const key = savedKey(draft);
  if (draft.profileRequest?.key === key) {
    draft.waitFor = draft.profileRequest.id;
    return;
  }
  if (draft.profileRequest) {
    draft.profileAttempt = null;
    draft.waitFor = draft.profileRequest.id;
    return;
  }
  draft.profileAttempt = null;
  draft.waitFor = startProfile(draft);
}

function finishHistory(draft: Draft, requestId: number, result: Result<InvocationRecord[]>) {
  const request = draft.historyRequest;
  if (!request || request.id !== requestId) return;
  draft.historyRequest = null;
  if (request.workspaceGeneration === draft.workspaceGeneration) {
    if (result.ok) {
      const unchanged =
        JSON.stringify(result.value) === JSON.stringify(draft.data.invocations) &&
        draft.data.invocationsError === null;
      if (!unchanged) draft.data.invocations = result.value;
      draft.data.invocationsError = null;
      refreshSavedAfterHistory(draft);
    } else {
      draft.data.invocationsError = result.error;
    }
  }
  if (draft.historyAgain) {
    draft.historyAgain = false;
    draft.historyAttempt = null;
  }
  syncAll(draft);
}

function finishSnapshot(draft: Draft, requestId: number, result: Result<TargetIndex | null>) {
  const request = draft.snapshotRequest;
  if (!request || request.id !== requestId) return;
  draft.snapshotRequest = null;
  const oldTargetKey = targetKey(draft);
  if (request.key === snapshotKey(draft)) {
    if (result.ok) {
      const selection = draft.data.selection;
      const targetValid =
        selection?.kind === "target" &&
        result.value?.targets.some((target) => target.label === selection.label);
      draft.data.snapshot = result.value;
      draft.data.snapshotError = null;
      if (draft.data.targetDetailError === TARGET_LIST_ERROR) draft.data.targetDetailError = null;
      if (selection?.kind === "package" || (selection?.kind === "target" && !targetValid)) {
        setSelection(draft, null);
      }
      resetTargetContext(draft, oldTargetKey);
    } else {
      draft.data.snapshotError = `Could not load the workspace snapshot: ${result.error}`;
      if (draft.data.selection?.kind === "target") draft.data.targetDetailError = TARGET_LIST_ERROR;
    }
  }
  syncAll(draft);
}

function finishTargetDetail(draft: Draft, requestId: number, result: Result<TargetDetail>) {
  const request = draft.targetDetailRequest;
  if (!request || request.id !== requestId) return;
  draft.targetDetailRequest = null;
  if (request.key === targetKey(draft)) {
    if (result.ok && result.value.target.label === request.label) {
      setData(draft, { targetDetail: result.value, targetDetailError: null });
    } else {
      draft.data.targetDetailError = result.ok
        ? "Error: Target detail does not match the selected label"
        : result.error;
    }
  }
  syncAll(draft);
}

function finishTargetOutputs(draft: Draft, requestId: number, result: Result<TargetOutputs>) {
  const request = draft.targetOutputsRequest;
  if (!request || request.id !== requestId) return;
  draft.targetOutputsRequest = null;
  if (request.key === targetKey(draft) && draft.data.targetDetail?.target.label === request.label) {
    setData(draft, {
      targetOutputs: result.ok ? result.value : null,
      targetOutputsError: result.ok ? null : result.error,
    });
  }
  syncAll(draft);
}

function finishSavedDetail(draft: Draft, requestId: number, result: Result<InvocationDetail>) {
  const request = draft.detailRequest;
  if (!request || request.id !== requestId) return;
  draft.detailRequest = null;
  if (request.key === savedKey(draft) && savedEligible(draft)) {
    if (result.ok) draft.data.detail = result.value;
    draft.data.detailError = result.ok ? null : result.error;
  }
  syncAll(draft);
}

function finishProfile(draft: Draft, requestId: number, result: Result<Section<Profile>>) {
  const request = draft.profileRequest;
  if (!request || request.id !== requestId) return;
  draft.profileRequest = null;
  if (request.key === savedKey(draft) && savedEligible(draft)) {
    if (result.ok) draft.data.profile = result.value;
    draft.data.profileError = result.ok ? null : result.error;
  }
  syncAll(draft);
}

function start(draft: Draft, cmd: BazelCommand, pattern: string, argumentsText: string) {
  if (!canSend(sessionView(draft), pattern) || !draft.data.state?.workspace) return;
  const request: StartRequest = {
    id: nextId(draft),
    workspace: draft.data.state.workspace,
    cmd,
    pattern,
    argumentsText,
    previousExecution: draft.observedExecution,
    observedId: null,
    record: null,
  };
  draft.pendingStart = request;
  draft.data.error = null;
  draft.effects.push({ type: "startInvocation", request });
  syncAll(draft);
  draft.waitFor = request.id;
}

function startResult(draft: Draft, requestId: number, result: Result<InvocationRecord>) {
  const pending = draft.pendingStart;
  if (!pending || pending.id !== requestId) return;
  if (!result.ok) {
    finishStart(draft, requestId);
    draft.data.error = result.error;
    syncAll(draft);
    return;
  }
  const record = result.value;
  draft.pendingStart = { ...pending, record };
  if (!draft.data.consoleOpen || draft.data.console?.id !== record.id) {
    setData(draft, {
      console: { ...record, cmd: pending.cmd, pattern: pending.pattern },
      consoleOpen: true,
    });
    setSelectedInvocation(draft, null);
  }
  const id = nextId(draft);
  draft.effects.push({
    type: "getStartState",
    id,
    startId: requestId,
    revision: draft.stateRevision,
  });
  draft.waitFor = id;
}

function startStateResult(
  draft: Draft,
  message: Extract<SessionMessage, { type: "startStateResult" }>,
) {
  const pending = draft.pendingStart;
  if (!pending || pending.id !== message.startId) return;
  if (message.revision === draft.stateRevision) {
    if (message.result.ok) {
      applyAppState(draft, message.result.value);
    } else {
      setData(draft, {
        ready: false,
        error: `The invocation started, but getting its state failed; waiting for the next update: ${message.result.error}`,
      });
    }
  }
  finishStart(draft, message.startId);
  draft.historyAttempt = null;
  if (draft.historyRequest) draft.historyAgain = true;
  syncAll(draft);
}

function openWorkspace(draft: Draft, path: string, scope: TargetScope) {
  const id = nextId(draft);
  if (sessionBusy(sessionView(draft))) {
    draft.effects.push({ type: "rejectWorkspaceOpen", id, message: OPEN_WORKSPACE_ERROR });
    draft.waitFor = id;
    return;
  }
  const request = { id, path, scope, revision: draft.stateRevision };
  draft.workspaceRequest = request;
  draft.effects.push({ type: "openWorkspace", request });
  draft.waitFor = request.id;
}

function updateSelection(draft: Draft, selection: ExplorerSelection) {
  const previousKey = targetKey(draft);
  if (sameSelection(draft.data.selection, selection)) {
    if (selection?.kind === "target" && draft.data.targetDetailError) {
      draft.targetDetailAttempt = null;
      draft.data.targetDetailError = null;
      syncAll(draft);
    }
    return;
  }
  setSelection(draft, selection);
  resetTargetContext(draft, previousKey);
  syncAll(draft);
}

function updateSelectedInvocation(draft: Draft, id: string | null) {
  const unchanged = id === draft.data.selectedInvocation;
  setSelectedInvocation(draft, id);
  draft.data.consoleOpen = false;
  if (unchanged) {
    forceDetail(draft);
    forceProfile(draft);
  }
  syncAll(draft);
}

export function update(model: SessionModel, message: SessionMessage): SessionTransition {
  const next = draft(model);
  switch (message.type) {
    case "subscriptionsFailed":
      next.data.error = `Could not connect: ${message.error}`;
      break;
    case "initialStateResult":
      if (message.revision === next.stateRevision) {
        if (message.result.ok) {
          applyAppState(next, message.result.value);
          syncAll(next, true);
        } else if (!next.data.ready) {
          next.data.error = `Could not connect: ${message.result.error}`;
        }
      }
      break;
    case "appState":
      applyAppState(next, message.state);
      syncAll(next, true);
      break;
    case "historyChanged":
    case "loadInvocations":
      loadHistory(next);
      break;
    case "invocationsResult":
      finishHistory(next, message.requestId, message.result);
      break;
    case "loadSnapshot":
      loadSnapshot(next);
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
      finishSavedDetail(next, message.requestId, message.result);
      break;
    case "profileResult":
      finishProfile(next, message.requestId, message.result);
      break;
    case "select":
      updateSelection(next, message.selection);
      break;
    case "selectInvocation":
      updateSelectedInvocation(next, message.id);
      break;
    case "openConsole": {
      const expected = next.data.state?.consoleInvocation ?? next.data.state?.activeInvocation;
      if (message.record.id !== expected) {
        updateSelectedInvocation(next, message.record.id);
      } else if (!next.data.consoleOpen || next.data.console?.id !== message.record.id) {
        setSelectedInvocation(next, null);
        setData(next, { console: message.record, consoleOpen: true });
        syncAll(next);
      }
      break;
    }
    case "loadDetail":
      forceDetail(next);
      break;
    case "loadProfile":
      forceProfile(next);
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
    case "openWorkspace":
      openWorkspace(next, message.path, message.scope);
      break;
    case "workspaceResult": {
      const request = next.workspaceRequest;
      if (request?.id === message.requestId) {
        next.workspaceRequest = null;
        if (message.result.ok && request.revision === next.stateRevision) {
          applyAppState(next, message.result.value);
          syncAll(next, true);
        } else {
          syncAll(next);
        }
      }
      break;
    }
    case "stop":
      if (canStop(sessionView(next))) {
        const id = nextId(next);
        next.effects.push({ type: "stop", id, workspace: next.data.state?.workspace ?? null });
        next.waitFor = id;
      }
      break;
    case "stopResult":
      if (!message.result.ok && message.workspace === next.data.state?.workspace) {
        next.data.error = message.result.error;
      }
      break;
    case "refresh":
      if (!sessionBusy(sessionView(next))) {
        const id = nextId(next);
        next.effects.push({ type: "refresh", id, workspace: next.data.state?.workspace ?? null });
        next.waitFor = id;
      }
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
