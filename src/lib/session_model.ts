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
  readonly key: string;
  readonly workspace: string;
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
  readonly workspace: string;
  readonly id: string;
  readonly active: boolean;
}

interface StartRequest {
  readonly id: number;
  readonly token: number;
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
  readonly startToken: number;
  readonly pendingStart: StartRequest | null;
  readonly observedExecution: ObservedExecution | null;
  readonly workspaceRequest: WorkspaceRequest | null;
  readonly subscriptionRequestId: number | null;
  readonly initialStateRequest: { readonly id: number; readonly revision: number } | null;
  readonly historyRequest: HistoryRequest | null;
  readonly historyAttempt: string | null;
  readonly historyAgain: boolean;
  readonly snapshotRequests: SnapshotRequest[];
  readonly snapshotAttempt: string | null;
  readonly targetDetailRequest: TargetRequest | null;
  readonly targetDetailAttempt: string | null;
  readonly targetDetailFor: string | null;
  readonly targetOutputsRequest: TargetRequest | null;
  readonly targetOutputsAttempt: string | null;
  readonly detailRequest: SavedRequest | null;
  readonly detailAttempt: string | null;
  readonly profileRequest: SavedRequest | null;
  readonly profileAttempt: string | null;
}

export type SessionEffect =
  | { readonly type: "subscribe"; readonly requestId: number }
  | { readonly type: "getInitialState"; readonly requestId: number; readonly revision: number }
  | {
      readonly type: "listInvocations";
      readonly requestId: number;
      readonly request: HistoryRequest;
    }
  | { readonly type: "getSnapshot"; readonly requestId: number; readonly request: SnapshotRequest }
  | {
      readonly type: "getTargetDetail";
      readonly requestId: number;
      readonly request: TargetRequest;
    }
  | {
      readonly type: "getTargetOutputs";
      readonly requestId: number;
      readonly request: TargetRequest;
    }
  | { readonly type: "getInvocation"; readonly requestId: number; readonly request: SavedRequest }
  | { readonly type: "getProfile"; readonly requestId: number; readonly request: SavedRequest }
  | { readonly type: "startInvocation"; readonly requestId: number; readonly request: StartRequest }
  | {
      readonly type: "getStartState";
      readonly requestId: number;
      readonly startRequestId: number;
      readonly token: number;
      readonly revision: number;
    }
  | {
      readonly type: "openWorkspace";
      readonly requestId: number;
      readonly request: WorkspaceRequest;
    }
  | { readonly type: "rejectWorkspaceOpen"; readonly requestId: number; readonly message: string }
  | { readonly type: "stop"; readonly requestId: number; readonly workspace: string | null }
  | { readonly type: "refresh"; readonly requestId: number; readonly workspace: string | null };

export type SessionMessage =
  | { readonly type: "init" }
  | { readonly type: "subscriptionsReady"; readonly requestId: number }
  | { readonly type: "subscriptionsFailed"; readonly requestId: number; readonly error: string }
  | {
      readonly type: "initialStateLoaded";
      readonly requestId: number;
      readonly revision: number;
      readonly state: AppState;
    }
  | {
      readonly type: "initialStateFailed";
      readonly requestId: number;
      readonly revision: number;
      readonly error: string;
    }
  | { readonly type: "appState"; readonly state: AppState }
  | { readonly type: "historyChanged" }
  | { readonly type: "loadInvocations" }
  | {
      readonly type: "invocationsLoaded";
      readonly requestId: number;
      readonly records: InvocationRecord[];
    }
  | { readonly type: "invocationsFailed"; readonly requestId: number; readonly error: string }
  | { readonly type: "loadSnapshot" }
  | {
      readonly type: "snapshotLoaded";
      readonly requestId: number;
      readonly snapshot: TargetIndex | null;
    }
  | { readonly type: "snapshotFailed"; readonly requestId: number; readonly error: string }
  | {
      readonly type: "targetDetailLoaded";
      readonly requestId: number;
      readonly detail: TargetDetail;
    }
  | { readonly type: "targetDetailFailed"; readonly requestId: number; readonly error: string }
  | {
      readonly type: "targetOutputsLoaded";
      readonly requestId: number;
      readonly outputs: TargetOutputs;
    }
  | { readonly type: "targetOutputsFailed"; readonly requestId: number; readonly error: string }
  | { readonly type: "detailLoaded"; readonly requestId: number; readonly detail: InvocationDetail }
  | { readonly type: "detailFailed"; readonly requestId: number; readonly error: string }
  | {
      readonly type: "profileLoaded";
      readonly requestId: number;
      readonly profile: Section<Profile>;
    }
  | { readonly type: "profileFailed"; readonly requestId: number; readonly error: string }
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
  | {
      readonly type: "startReturned";
      readonly requestId: number;
      readonly token: number;
      readonly record: InvocationRecord;
    }
  | {
      readonly type: "startFailed";
      readonly requestId: number;
      readonly token: number;
      readonly error: string;
    }
  | {
      readonly type: "startStateLoaded";
      readonly requestId: number;
      readonly startRequestId: number;
      readonly token: number;
      readonly revision: number;
      readonly state: AppState;
    }
  | {
      readonly type: "startStateFailed";
      readonly requestId: number;
      readonly startRequestId: number;
      readonly token: number;
      readonly revision: number;
      readonly error: string;
    }
  | { readonly type: "openWorkspace"; readonly path: string; readonly scope: TargetScope }
  | {
      readonly type: "workspaceOpened";
      readonly requestId: number;
      readonly revision: number;
      readonly state: AppState;
    }
  | { readonly type: "workspaceOpenFailed"; readonly requestId: number }
  | { readonly type: "stop" }
  | { readonly type: "stopFailed"; readonly workspace: string | null; readonly error: string }
  | { readonly type: "refresh" }
  | { readonly type: "refreshFailed"; readonly workspace: string | null; readonly error: string };

export interface SessionTransition {
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
    startToken: 0,
    pendingStart: null,
    observedExecution: null,
    workspaceRequest: null,
    subscriptionRequestId: null,
    initialStateRequest: null,
    historyRequest: null,
    historyAttempt: null,
    historyAgain: false,
    snapshotRequests: [],
    snapshotAttempt: null,
    targetDetailRequest: null,
    targetDetailAttempt: null,
    targetDetailFor: null,
    targetOutputsRequest: null,
    targetOutputsAttempt: null,
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
    snapshotLoading: model.snapshotRequests.some((request) => request.key === snapshotKey(model)),
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

function result(
  model: SessionModel,
  effects: SessionEffect[] = [],
  waitFor?: number,
): SessionTransition {
  return { model, effects, waitFor };
}

function unexpected(message: never): never {
  throw new Error(`Unhandled session message: ${JSON.stringify(message)}`);
}

function withData(model: SessionModel, patch: Partial<SessionData>): SessionModel {
  return { ...model, data: { ...model.data, ...patch } };
}

function reserveId(model: SessionModel): { readonly id: number; readonly model: SessionModel } {
  return { id: model.nextRequestId, model: { ...model, nextRequestId: model.nextRequestId + 1 } };
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

function setSelection(model: SessionModel, selection: ExplorerSelection): SessionModel {
  if (sameSelection(model.data.selection, selection)) return model;
  return {
    ...model,
    selectionGeneration: model.selectionGeneration + 1,
    data: { ...model.data, selection },
  };
}

function setSelectedInvocation(model: SessionModel, id: string | null): SessionModel {
  if (model.data.selectedInvocation === id) return model;
  return {
    ...model,
    invocationGeneration: model.invocationGeneration + 1,
    data: {
      ...model.data,
      selectedInvocation: id,
      detail: null,
      detailError: null,
      profile: null,
      profileError: null,
    },
  };
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

function resetTargetContext(model: SessionModel, previousKey: string): SessionModel {
  if (previousKey === targetKey(model)) return model;
  return {
    ...model,
    targetDetailAttempt: null,
    targetDetailFor: null,
    targetOutputsAttempt: null,
    data: {
      ...model.data,
      targetDetail: null,
      targetDetailError: null,
      targetOutputs: null,
      targetOutputsError: null,
    },
  };
}

function syncHistory(model: SessionModel): SessionTransition {
  const key = historyKey(model);
  const prepared = model.historyAttempt !== key ? { ...model, historyAttempt: null } : model;
  const workspace = model.data.state?.workspace;
  if (!workspace || !prepared.data.ready || prepared.historyRequest) return result(prepared);
  if (prepared.historyAttempt === key) return result(prepared);
  const reserved = reserveId(prepared);
  const request: HistoryRequest = {
    id: reserved.id,
    key,
    workspace,
    workspaceGeneration: model.workspaceGeneration,
  };
  const next = {
    ...reserved.model,
    historyRequest: request,
    historyAttempt: key,
    data: { ...reserved.model.data, invocationsError: null },
  };
  return result(next, [{ type: "listInvocations", requestId: request.id, request }]);
}

function syncSnapshot(model: SessionModel, authoritative = false): SessionTransition {
  const state = model.data.state;
  const key = snapshotKey(model);
  let next = model;
  if (next.snapshotAttempt !== key) {
    next = { ...next, snapshotAttempt: null };
    if (next.data.snapshotError !== null) next = withData(next, { snapshotError: null });
  }
  if (!state?.workspace || state.snapshotFetchedAt === null) return result(next);
  if (authoritative && next.data.snapshotError !== null) {
    next = { ...next, snapshotAttempt: null };
  }
  if (next.snapshotRequests.some((request) => request.key === key)) return result(next);
  if (next.snapshotAttempt === key) return result(next);
  const reserved = reserveId(next);
  const request: SnapshotRequest = { id: reserved.id, key };
  next = {
    ...reserved.model,
    snapshotRequests: [...reserved.model.snapshotRequests, request],
    snapshotAttempt: key,
    data: { ...reserved.model.data, snapshotError: null },
  };
  return result(next, [{ type: "getSnapshot", requestId: request.id, request }]);
}

function startTargetDetail(model: SessionModel): SessionTransition {
  if (!targetReady(model) || model.targetDetailRequest) return result(model);
  const selection = model.data.selection;
  const fetchedAt = model.data.state?.snapshotFetchedAt;
  if (selection?.kind !== "target" || fetchedAt === null || fetchedAt === undefined) {
    return result(model);
  }
  const key = targetKey(model);
  if (model.targetDetailAttempt === key) return result(model);
  const reserved = reserveId(model);
  const request: TargetRequest = { id: reserved.id, key, label: selection.label, fetchedAt };
  const next = {
    ...reserved.model,
    targetDetailRequest: request,
    targetDetailAttempt: key,
    data: { ...reserved.model.data, targetDetailError: null },
  };
  return result(next, [{ type: "getTargetDetail", requestId: request.id, request }]);
}

function startTargetOutputs(model: SessionModel): SessionTransition {
  const key = targetKey(model);
  if (
    !targetReady(model) ||
    model.targetDetailFor !== key ||
    model.targetOutputsRequest ||
    model.targetOutputsAttempt === key
  ) {
    return result(model);
  }
  const selection = model.data.selection;
  const fetchedAt = model.data.state?.snapshotFetchedAt;
  if (selection?.kind !== "target" || fetchedAt === null || fetchedAt === undefined) {
    return result(model);
  }
  const reserved = reserveId(model);
  const request: TargetRequest = { id: reserved.id, key, label: selection.label, fetchedAt };
  const next = {
    ...reserved.model,
    targetOutputsRequest: request,
    targetOutputsAttempt: key,
    data: { ...reserved.model.data, targetOutputsError: null },
  };
  return result(next, [{ type: "getTargetOutputs", requestId: request.id, request }]);
}

function startDetail(model: SessionModel): SessionTransition {
  const key = savedKey(model);
  if (!savedEligible(model) || model.detailRequest || model.detailAttempt === key) {
    return result(model);
  }
  const invocationId = model.data.selectedInvocation;
  if (!invocationId) return result(model);
  const reserved = reserveId(model);
  const request: SavedRequest = { id: reserved.id, key, invocationId };
  const next = {
    ...reserved.model,
    detailRequest: request,
    detailAttempt: key,
    data: { ...reserved.model.data, detailError: null },
  };
  return result(next, [{ type: "getInvocation", requestId: request.id, request }]);
}

function startProfile(model: SessionModel): SessionTransition {
  const key = savedKey(model);
  if (!savedEligible(model) || model.profileRequest || model.profileAttempt === key) {
    return result(model);
  }
  const invocationId = model.data.selectedInvocation;
  if (!invocationId) return result(model);
  const reserved = reserveId(model);
  const request: SavedRequest = { id: reserved.id, key, invocationId };
  const next = {
    ...reserved.model,
    profileRequest: request,
    profileAttempt: key,
    data: { ...reserved.model.data, profileError: null },
  };
  return result(next, [{ type: "getProfile", requestId: request.id, request }]);
}

function syncAll(model: SessionModel, authoritative = false): SessionTransition {
  let next = model;
  const effects: SessionEffect[] = [];
  let transition = syncHistory(next);
  next = transition.model;
  effects.push(...transition.effects);
  transition = syncSnapshot(next, authoritative);
  next = transition.model;
  effects.push(...transition.effects);
  transition = startTargetDetail(next);
  next = transition.model;
  effects.push(...transition.effects);
  transition = startTargetOutputs(next);
  next = transition.model;
  effects.push(...transition.effects);
  transition = syncSaved(next);
  next = transition.model;
  effects.push(...transition.effects);
  return result(next, effects);
}

function syncSaved(model: SessionModel): SessionTransition {
  const key = savedKey(model);
  let next = model;
  if (next.detailAttempt !== key) {
    next = {
      ...next,
      detailAttempt: null,
      data: next.data.detailError === null ? next.data : { ...next.data, detailError: null },
    };
  }
  if (next.profileAttempt !== key) {
    next = {
      ...next,
      profileAttempt: null,
      data: next.data.profileError === null ? next.data : { ...next.data, profileError: null },
    };
  }
  let transition = startDetail(next);
  next = transition.model;
  const effects = [...transition.effects];
  transition = startProfile(next);
  next = transition.model;
  effects.push(...transition.effects);
  return result(next, effects);
}

function refreshSavedAfterHistory(model: SessionModel): SessionModel {
  const record = selectedRecord(model);
  if (
    record &&
    model.data.detail &&
    !sessionView(model).detailLoading &&
    model.data.detailError === null &&
    JSON.stringify(record) !== JSON.stringify(model.data.detail.record)
  ) {
    return {
      ...model,
      detailAttempt: null,
      profileAttempt: null,
      data: { ...model.data, profileError: null },
    };
  }
  return model;
}

function applyAppState(model: SessionModel, state: AppState): SessionModel {
  const before = model.data.state;
  const workspaceChanged = state.workspace !== before?.workspace;
  const scopeChanged =
    JSON.stringify(state.scope.include) !== JSON.stringify(before?.scope.include);
  const oldTargetKey = targetKey(model);
  const resumed = !model.data.ready;
  let next: SessionModel = {
    ...model,
    stateRevision: model.stateRevision + 1,
    data: {
      ...model.data,
      state,
      ready: true,
      ...(resumed ? { error: null } : {}),
    },
  };

  if (workspaceChanged) {
    next = {
      ...next,
      workspaceGeneration: next.workspaceGeneration + 1,
      startToken: next.startToken + 1,
      pendingStart: null,
      historyAgain: false,
      observedExecution: state.activeInvocation
        ? { workspace: state.workspace ?? "", id: state.activeInvocation, active: true }
        : null,
      data: {
        ...next.data,
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
      },
      snapshotAttempt: null,
    };
    next = setSelectedInvocation(next, null);
    next = setSelection(next, null);
  } else {
    if (scopeChanged || state.snapshotFetchedAt === null) {
      next = setSelection(next, null);
    }
    if (scopeChanged || state.snapshotFetchedAt === null) {
      next = {
        ...next,
        snapshotAttempt: null,
        data: { ...next.data, snapshot: null, snapshotError: null },
      };
    }

    const starting = next.pendingStart;
    if (starting) {
      if (
        starting.workspace === state.workspace &&
        state.activeInvocation &&
        state.consoleInvocation === state.activeInvocation &&
        starting.observedId !== state.activeInvocation
      ) {
        next = {
          ...next,
          pendingStart: { ...starting, observedId: state.activeInvocation },
          data: {
            ...next.data,
            console: { id: state.activeInvocation, cmd: starting.cmd, pattern: starting.pattern },
            consoleOpen: true,
          },
        };
        next = setSelectedInvocation(next, null);
      }
    } else {
      const observed = next.observedExecution;
      const activeId = state.activeInvocation;
      let completedId: string | null = null;
      if (activeId) {
        if (observed && observed.active && before?.activeInvocation === observed.id) {
          if (observed.id !== activeId) completedId = observed.id;
        }
        next = {
          ...next,
          observedExecution: { workspace: state.workspace ?? "", id: activeId, active: true },
        };
      } else {
        if (
          observed &&
          ((observed.active && before?.activeInvocation === observed.id) ||
            state.consoleInvocation === observed.id)
        ) {
          completedId = observed.id;
        }
        next = { ...next, observedExecution: null };
      }
      if (completedId) next = completeConsole(next, completedId);
    }
  }

  return resetTargetContext(next, oldTargetKey);
}

function completeConsole(model: SessionModel, id: string): SessionModel {
  if (
    model.pendingStart ||
    model.data.state?.activeInvocation ||
    !model.data.consoleOpen ||
    model.data.console?.id !== id
  ) {
    return model;
  }
  return setSelectedInvocation(withData(model, { consoleOpen: false }), id);
}

function finishStart(model: SessionModel, requestId: number, token: number): SessionModel {
  const current = model.pendingStart;
  if (
    !current ||
    current.id !== requestId ||
    current.token !== token ||
    token !== model.startToken
  ) {
    return model;
  }
  if (!current.record) {
    return { ...model, pendingStart: null, observedExecution: current.previousExecution };
  }
  const id = current.record.id;
  let observedExecution: ObservedExecution | null = null;
  let completed = false;
  if (!model.data.ready) {
    observedExecution = {
      workspace: current.workspace,
      id,
      active: current.observedId === id,
    };
  } else if (model.data.state?.activeInvocation === id) {
    observedExecution = { workspace: current.workspace, id, active: true };
  } else if (!model.data.state?.activeInvocation) {
    completed = true;
  } else {
    observedExecution = {
      workspace: model.data.state.workspace ?? current.workspace,
      id: model.data.state.activeInvocation,
      active: true,
    };
  }
  let next: SessionModel = { ...model, pendingStart: null, observedExecution };
  if (completed) next = completeConsole(next, id);
  return next;
}

function loadHistory(model: SessionModel): SessionTransition {
  if (!model.data.state?.workspace || !model.data.ready) return result(model);
  if (model.historyRequest) {
    return result(
      model.historyAgain ? model : { ...model, historyAgain: true },
      [],
      model.historyRequest.id,
    );
  }
  const transition = syncHistory({ ...model, historyAttempt: null });
  return result(transition.model, transition.effects, transition.model.historyRequest?.id);
}

function loadSnapshot(model: SessionModel): SessionTransition {
  const key = snapshotKey(model);
  const inFlight = model.snapshotRequests.find((request) => request.key === key);
  if (inFlight) return result(model, [], inFlight.id);
  if (!model.data.state?.workspace || model.data.state.snapshotFetchedAt === null) {
    return result(model);
  }
  const transition = syncSnapshot({ ...model, snapshotAttempt: null });
  return result(
    transition.model,
    transition.effects,
    transition.model.snapshotRequests.find((request) => request.key === key)?.id,
  );
}

function forceDetail(model: SessionModel): SessionTransition {
  if (!savedEligible(model)) return result(model);
  const key = savedKey(model);
  if (model.detailRequest?.key === key) return result(model, [], model.detailRequest.id);
  if (model.detailRequest) {
    return result({ ...model, detailAttempt: null }, [], model.detailRequest.id);
  }
  const transition = startDetail({ ...model, detailAttempt: null });
  return result(transition.model, transition.effects, transition.model.detailRequest?.id);
}

function forceProfile(model: SessionModel): SessionTransition {
  if (!savedEligible(model)) return result(model);
  const key = savedKey(model);
  if (model.profileRequest?.key === key) return result(model, [], model.profileRequest.id);
  if (model.profileRequest) {
    return result({ ...model, profileAttempt: null }, [], model.profileRequest.id);
  }
  const transition = startProfile({ ...model, profileAttempt: null });
  return result(transition.model, transition.effects, transition.model.profileRequest?.id);
}

function finishHistory(
  model: SessionModel,
  requestId: number,
  records?: InvocationRecord[],
  error?: string,
): SessionTransition {
  const request = model.historyRequest;
  if (!request || request.id !== requestId) return result(model);
  let next: SessionModel = { ...model, historyRequest: null };
  const accepted =
    request.workspaceGeneration === model.workspaceGeneration &&
    request.workspace === model.data.state?.workspace;
  if (accepted && records) {
    const unchanged =
      JSON.stringify(records) === JSON.stringify(model.data.invocations) &&
      model.data.invocationsError === null;
    next = withData(next, {
      ...(unchanged ? {} : { invocations: records }),
      invocationsError: null,
    });
    next = refreshSavedAfterHistory(next);
  } else if (accepted && error !== undefined) {
    next = withData(next, { invocationsError: error });
  }
  if (model.historyAgain) {
    next = { ...next, historyAgain: false, historyAttempt: null };
  }
  return syncAll(next);
}

function finishSnapshot(
  model: SessionModel,
  requestId: number,
  snapshot?: TargetIndex | null,
  error?: string,
): SessionTransition {
  const request = model.snapshotRequests.find((item) => item.id === requestId);
  if (!request) return result(model);
  const oldTargetKey = targetKey(model);
  let next: SessionModel = {
    ...model,
    snapshotRequests: model.snapshotRequests.filter((item) => item.id !== requestId),
  };
  if (request.key === snapshotKey(model) && snapshot !== undefined) {
    const selection = next.data.selection;
    const targetValid =
      selection?.kind === "target" &&
      snapshot?.targets.some((target) => target.label === selection.label);
    next = withData(next, {
      snapshot,
      snapshotError: null,
      ...(next.data.targetDetailError === TARGET_LIST_ERROR ? { targetDetailError: null } : {}),
    });
    if (selection?.kind === "package" || (selection?.kind === "target" && !targetValid)) {
      next = setSelection(next, null);
    }
    next = resetTargetContext(next, oldTargetKey);
  } else if (request.key === snapshotKey(model) && error !== undefined) {
    next = withData(next, {
      snapshotError: `Could not load the workspace snapshot: ${error}`,
      ...(next.data.selection?.kind === "target" ? { targetDetailError: TARGET_LIST_ERROR } : {}),
    });
  }
  return syncAll(next);
}

function finishTargetDetail(
  model: SessionModel,
  requestId: number,
  detail?: TargetDetail,
  error?: string,
): SessionTransition {
  const request = model.targetDetailRequest;
  if (!request || request.id !== requestId) return result(model);
  let next: SessionModel = { ...model, targetDetailRequest: null };
  if (request.key === targetKey(model)) {
    if (detail && detail.target.label === request.label) {
      next = {
        ...next,
        targetDetailFor: request.key,
        data: { ...next.data, targetDetail: detail, targetDetailError: null },
      };
    } else {
      next = withData(next, {
        targetDetailError: error ?? "Error: Target detail does not match the selected label",
      });
    }
  }
  return syncAll(next);
}

function finishTargetOutputs(
  model: SessionModel,
  requestId: number,
  outputs?: TargetOutputs,
  error?: string,
): SessionTransition {
  const request = model.targetOutputsRequest;
  if (!request || request.id !== requestId) return result(model);
  let next: SessionModel = { ...model, targetOutputsRequest: null };
  if (request.key === targetKey(model) && model.targetDetailFor === request.key) {
    next = withData(next, {
      targetOutputs: outputs ?? null,
      targetOutputsError: error ?? null,
    });
  }
  return syncAll(next);
}

function finishSavedDetail(
  model: SessionModel,
  requestId: number,
  detail?: InvocationDetail,
  error?: string,
): SessionTransition {
  const request = model.detailRequest;
  if (!request || request.id !== requestId) return result(model);
  let next: SessionModel = { ...model, detailRequest: null };
  if (request.key === savedKey(model) && savedEligible(model)) {
    next = withData(next, {
      ...(detail ? { detail } : {}),
      detailError: error ?? null,
    });
  }
  return syncAll(next);
}

function finishProfile(
  model: SessionModel,
  requestId: number,
  profile?: Section<Profile>,
  error?: string,
): SessionTransition {
  const request = model.profileRequest;
  if (!request || request.id !== requestId) return result(model);
  let next: SessionModel = { ...model, profileRequest: null };
  if (request.key === savedKey(model) && savedEligible(model)) {
    next = withData(next, {
      ...(profile ? { profile } : {}),
      profileError: error ?? null,
    });
  }
  return syncAll(next);
}

function start(
  cmd: BazelCommand,
  pattern: string,
  argumentsText: string,
  model: SessionModel,
): SessionTransition {
  if (!canSend(sessionView(model), pattern)) return result(model);
  const workspace = model.data.state?.workspace;
  if (!workspace) return result(model);
  const reserved = reserveId(model);
  const token = reserved.model.startToken + 1;
  const request: StartRequest = {
    id: reserved.id,
    token,
    workspace,
    cmd,
    pattern,
    argumentsText,
    previousExecution: model.observedExecution,
    observedId: null,
    record: null,
  };
  const next = withData(
    { ...reserved.model, startToken: token, pendingStart: request },
    { error: null },
  );
  const transition = syncAll(next);
  return result(
    transition.model,
    [{ type: "startInvocation", requestId: request.id, request }, ...transition.effects],
    request.id,
  );
}

function startReturned(
  model: SessionModel,
  requestId: number,
  token: number,
  record: InvocationRecord,
): SessionTransition {
  const pending = model.pendingStart;
  if (
    !pending ||
    pending.id !== requestId ||
    pending.token !== token ||
    model.startToken !== token
  ) {
    return result(model);
  }
  let next: SessionModel = {
    ...model,
    pendingStart: { ...pending, record },
  };
  if (!next.data.consoleOpen || next.data.console?.id !== record.id) {
    next = withData(next, {
      console: { ...record, cmd: pending.cmd, pattern: pending.pattern },
      consoleOpen: true,
    });
    next = setSelectedInvocation(next, null);
  }
  const reserved = reserveId(next);
  return result(
    reserved.model,
    [
      {
        type: "getStartState",
        requestId: reserved.id,
        startRequestId: requestId,
        token,
        revision: next.stateRevision,
      },
    ],
    reserved.id,
  );
}

function openWorkspace(model: SessionModel, path: string, scope: TargetScope): SessionTransition {
  const reserved = reserveId(model);
  if (sessionBusy(sessionView(model))) {
    return result(
      reserved.model,
      [
        {
          type: "rejectWorkspaceOpen",
          requestId: reserved.id,
          message: OPEN_WORKSPACE_ERROR,
        },
      ],
      reserved.id,
    );
  }
  const request: WorkspaceRequest = {
    id: reserved.id,
    path,
    scope,
    revision: model.stateRevision,
  };
  return result(
    { ...reserved.model, workspaceRequest: request },
    [{ type: "openWorkspace", requestId: request.id, request }],
    request.id,
  );
}

function updateSelection(model: SessionModel, selection: ExplorerSelection): SessionTransition {
  const previousKey = targetKey(model);
  if (sameSelection(model.data.selection, selection)) {
    if (selection?.kind === "target" && model.data.targetDetailError) {
      const next = {
        ...model,
        targetDetailAttempt: null,
        data: { ...model.data, targetDetailError: null },
      };
      return syncAll(next);
    }
    return result(model);
  }
  const next = resetTargetContext(setSelection(model, selection), previousKey);
  return syncAll(next);
}

function updateSelectedInvocation(model: SessionModel, id: string | null): SessionTransition {
  const unchanged = id === model.data.selectedInvocation;
  let next = setSelectedInvocation(model, id);
  next = withData(next, { consoleOpen: false });
  if (unchanged) {
    next = { ...next, detailAttempt: null, profileAttempt: null };
  }
  return syncAll(next);
}

export function update(model: SessionModel, message: SessionMessage): SessionTransition {
  switch (message.type) {
    case "init": {
      if (model.subscriptionRequestId !== null) return result(model);
      const reserved = reserveId(model);
      const next = { ...reserved.model, subscriptionRequestId: reserved.id };
      return result(next, [{ type: "subscribe", requestId: reserved.id }], reserved.id);
    }
    case "subscriptionsReady": {
      if (message.requestId !== model.subscriptionRequestId) return result(model);
      const reserved = reserveId(model);
      const request = { id: reserved.id, revision: model.stateRevision };
      const next = { ...reserved.model, initialStateRequest: request };
      return result(
        next,
        [
          {
            type: "getInitialState",
            requestId: request.id,
            revision: request.revision,
          },
        ],
        request.id,
      );
    }
    case "subscriptionsFailed": {
      if (message.requestId !== model.subscriptionRequestId) return result(model);
      return result(
        withData(
          { ...model, subscriptionRequestId: null },
          { error: `Could not connect: ${message.error}` },
        ),
      );
    }
    case "initialStateLoaded": {
      const request = model.initialStateRequest;
      if (!request || request.id !== message.requestId) return result(model);
      let next: SessionModel = { ...model, initialStateRequest: null };
      if (message.revision === model.stateRevision) {
        next = applyAppState(next, message.state);
        return syncAll(next, true);
      }
      return result(next);
    }
    case "initialStateFailed": {
      const request = model.initialStateRequest;
      if (!request || request.id !== message.requestId) return result(model);
      const next = { ...model, initialStateRequest: null };
      return result(
        !model.data.ready && request.revision === message.revision
          ? withData(next, { error: `Could not connect: ${message.error}` })
          : next,
      );
    }
    case "appState": {
      return syncAll(applyAppState(model, message.state), true);
    }
    case "historyChanged":
    case "loadInvocations": {
      return loadHistory(model);
    }
    case "invocationsLoaded": {
      return finishHistory(model, message.requestId, message.records);
    }
    case "invocationsFailed": {
      return finishHistory(model, message.requestId, undefined, message.error);
    }
    case "loadSnapshot": {
      return loadSnapshot(model);
    }
    case "snapshotLoaded": {
      return finishSnapshot(model, message.requestId, message.snapshot);
    }
    case "snapshotFailed": {
      return finishSnapshot(model, message.requestId, undefined, message.error);
    }
    case "targetDetailLoaded": {
      return finishTargetDetail(model, message.requestId, message.detail);
    }
    case "targetDetailFailed": {
      return finishTargetDetail(model, message.requestId, undefined, message.error);
    }
    case "targetOutputsLoaded": {
      return finishTargetOutputs(model, message.requestId, message.outputs);
    }
    case "targetOutputsFailed": {
      return finishTargetOutputs(model, message.requestId, undefined, message.error);
    }
    case "detailLoaded": {
      return finishSavedDetail(model, message.requestId, message.detail);
    }
    case "detailFailed": {
      return finishSavedDetail(model, message.requestId, undefined, message.error);
    }
    case "profileLoaded": {
      return finishProfile(model, message.requestId, message.profile);
    }
    case "profileFailed": {
      return finishProfile(model, message.requestId, undefined, message.error);
    }
    case "select": {
      return updateSelection(model, message.selection);
    }
    case "selectInvocation": {
      return updateSelectedInvocation(model, message.id);
    }
    case "openConsole": {
      const expected = model.data.state?.consoleInvocation ?? model.data.state?.activeInvocation;
      if (message.record.id !== expected) {
        return updateSelectedInvocation(model, message.record.id);
      }
      if (model.data.consoleOpen && model.data.console?.id === message.record.id) {
        return result(model);
      }
      const next = withData(setSelectedInvocation(model, null), {
        console: message.record,
        consoleOpen: true,
      });
      return syncAll(next);
    }
    case "loadDetail": {
      return forceDetail(model);
    }
    case "loadProfile": {
      return forceProfile(model);
    }
    case "start": {
      return start(message.cmd, message.pattern, message.argumentsText, model);
    }
    case "startReturned": {
      return startReturned(model, message.requestId, message.token, message.record);
    }
    case "startFailed": {
      const pending = model.pendingStart;
      if (
        !pending ||
        pending.id !== message.requestId ||
        pending.token !== message.token ||
        model.startToken !== message.token
      ) {
        return result(model);
      }
      const next = withData(finishStart(model, message.requestId, message.token), {
        error: message.error,
      });
      return syncAll(next);
    }
    case "startStateLoaded": {
      const pending = model.pendingStart;
      if (
        !pending ||
        pending.id !== message.startRequestId ||
        pending.token !== message.token ||
        model.startToken !== message.token
      ) {
        return result(model);
      }
      let next = model;
      if (message.revision === model.stateRevision) {
        next = applyAppState(next, message.state);
      }
      next = finishStart(next, message.startRequestId, message.token);
      return syncAll(next);
    }
    case "startStateFailed": {
      const pending = model.pendingStart;
      if (
        !pending ||
        pending.id !== message.startRequestId ||
        pending.token !== message.token ||
        model.startToken !== message.token
      ) {
        return result(model);
      }
      let next = model;
      if (message.revision === model.stateRevision) {
        next = withData(next, {
          ready: false,
          error: `The invocation started, but getting its state failed; waiting for the next update: ${message.error}`,
        });
      }
      next = finishStart(next, message.startRequestId, message.token);
      return syncAll(next);
    }
    case "openWorkspace": {
      return openWorkspace(model, message.path, message.scope);
    }
    case "workspaceOpened": {
      const request = model.workspaceRequest;
      if (!request || request.id !== message.requestId) return result(model);
      let next: SessionModel = { ...model, workspaceRequest: null };
      if (request.revision === model.stateRevision) {
        next = applyAppState(next, message.state);
      }
      return syncAll(next, true);
    }
    case "workspaceOpenFailed": {
      if (model.workspaceRequest?.id !== message.requestId) return result(model);
      return result({ ...model, workspaceRequest: null });
    }
    case "stop": {
      if (!canStop(sessionView(model))) return result(model);
      const reserved = reserveId(model);
      return result(
        reserved.model,
        [
          {
            type: "stop",
            requestId: reserved.id,
            workspace: model.data.state?.workspace ?? null,
          },
        ],
        reserved.id,
      );
    }
    case "stopFailed": {
      return result(
        message.workspace === model.data.state?.workspace
          ? withData(model, { error: message.error })
          : model,
      );
    }
    case "refresh": {
      if (sessionBusy(sessionView(model))) return result(model);
      const reserved = reserveId(model);
      return result(
        reserved.model,
        [
          {
            type: "refresh",
            requestId: reserved.id,
            workspace: model.data.state?.workspace ?? null,
          },
        ],
        reserved.id,
      );
    }
    case "refreshFailed": {
      return result(
        message.workspace === model.data.state?.workspace
          ? withData(model, { error: message.error })
          : model,
      );
    }
    default: {
      return unexpected(message);
    }
  }
}
