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
  TargetOutputs,
  TargetScope,
  WorkspaceState,
} from "./types.ts";

export type WorkspacePage = "targets" | "analytics" | "query";

export type SessionContent =
  | { readonly kind: WorkspacePage }
  | { readonly kind: "invocation"; readonly id: string; readonly page: WorkspacePage }
  | { readonly kind: "console"; readonly id: string; readonly page: WorkspacePage };

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

interface StartRequest {
  readonly workspace: string;
  readonly navigation: number;
}

export interface SessionModel {
  readonly content: SessionContent;
  readonly connection: {
    state: AppState | null;
    error: string | null;
  };
  readonly workspace: string | null;
  readonly navigation: number;
  readonly workspacePrompt: {
    status: "closed" | "editing" | "opening";
    error: string | null;
    requestId: number | null;
  };
  readonly starts: Record<number, StartRequest>;
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
  core: AppState | null;
  state: WorkspaceState | null;
  executions: OwnedInvocation[];
  execution: OwnedInvocation | null;
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
  | { readonly type: "listInvocations"; readonly id: number; readonly workspace: string }
  | { readonly type: "getSnapshot"; readonly id: number; readonly workspace: string }
  | {
      readonly type: "getTargetDetail";
      readonly id: number;
      readonly workspace: string;
      readonly label: string;
      readonly fetchedAt: number;
    }
  | {
      readonly type: "getTargetOutputs";
      readonly id: number;
      readonly workspace: string;
      readonly label: string;
      readonly fetchedAt: number;
    }
  | {
      readonly type: "getTargetActions";
      readonly id: number;
      readonly workspace: string;
      readonly label: string;
      readonly fetchedAt: number;
    }
  | {
      readonly type: "getInvocation";
      readonly id: number;
      readonly workspace: string;
      readonly invocationId: string;
    }
  | {
      readonly type: "getProfile";
      readonly id: number;
      readonly workspace: string;
      readonly invocationId: string;
    }
  | {
      readonly type: "startInvocation";
      readonly id: number;
      readonly workspace: string;
      readonly cmd: BazelCommand;
      readonly pattern: string;
      readonly argumentsText: string;
    }
  | {
      readonly type: "openWorkspace";
      readonly id: number;
      readonly path: string;
      readonly scope: TargetScope;
    }
  | { readonly type: "stop"; readonly id: string; readonly workspace: string }
  | { readonly type: "refresh"; readonly workspace: string };

export type SessionMessage =
  | { readonly type: "subscriptionsFailed"; readonly error: string }
  | ResultMessage<"stateResult", AppState>
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
  | { readonly type: "openConsole"; readonly id: string }
  | { readonly type: "loadDetail" }
  | { readonly type: "loadProfile" }
  | {
      readonly type: "start";
      readonly cmd: BazelCommand;
      readonly pattern: string;
      readonly argumentsText: string;
    }
  | RequestResult<"startResult", OwnedInvocation>
  | { readonly type: "workspacePrompt"; readonly open: boolean }
  | { readonly type: "openWorkspace"; readonly path: string; readonly scope: TargetScope }
  | RequestResult<"workspaceResult", OpenWorkspaceResult>
  | { readonly type: "stop"; readonly id: string }
  | (ResultMessage<"stopResult", void> & { readonly id: string })
  | { readonly type: "refresh" }
  | (ResultMessage<"refreshResult", void> & { readonly workspace: string });

interface SessionTransition {
  readonly model: SessionModel;
  readonly effects: SessionEffect[];
}

const TARGET_LIST_ERROR = "Could not load the target list";

function emptyReader<T>(): Reader<T> {
  return { value: null, error: null, running: null, wanted: null };
}

export function initialModel(page: WorkspacePage = "targets"): SessionModel {
  return {
    content: { kind: page },
    connection: { state: null, error: null },
    workspace: null,
    navigation: 0,
    workspacePrompt: { status: "closed", error: null, requestId: null },
    starts: {},
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

function wsEntry(state: AppState | null, workspace: string | null): WorkspaceState | null {
  if (!state || workspace === null) return null;
  return state.workspaces.find((entry) => entry.workspace === workspace) ?? null;
}

function ownedEntry(state: AppState | null, id: string | null): OwnedInvocation | null {
  if (!state || id === null) return null;
  return state.invocations.find((entry) => entry.id === id) ?? null;
}

function selectedInvocation(model: SessionModel): string | null {
  const content = model.content;
  return content.kind === "invocation" ? content.id : null;
}

function contentInvocationId(model: SessionModel): string | null {
  const content = model.content;
  return content.kind === "invocation" || content.kind === "console" ? content.id : null;
}

function workspacePage(model: SessionModel): WorkspacePage {
  const content = model.content;
  return content.kind === "invocation" || content.kind === "console" ? content.page : content.kind;
}

function savedEligible(model: SessionModel): boolean {
  const id = selectedInvocation(model);
  return !!(
    id &&
    model.connection.state !== null &&
    model.workspace &&
    model.history.records.some((record) => record.id === id)
  );
}

function consoleRecord(model: SessionModel, core: AppState | null): InvocationRecord | null {
  if (model.content.kind !== "console") return null;
  const id = model.content.id;
  return recordFor(model.history.records, id) ?? ownedEntry(core, id);
}

export function sessionView(model: SessionModel): SessionView {
  const eligible = savedEligible(model);
  const core = model.connection.state;
  const historyIds = new Set(model.history.records.map((record) => record.id));
  const executions = (core?.invocations ?? [])
    .filter(
      (entry) =>
        entry.workspace === model.workspace &&
        (entry.running || (entry.console && !historyIds.has(entry.id))),
    )
    .sort((a, b) => b.startedAt - a.startedAt);
  return {
    content: model.content,
    console: consoleRecord(model, core),
    core,
    state: wsEntry(core, model.workspace),
    executions,
    execution: ownedEntry(core, contentInvocationId(model)),
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
    ready: core !== null,
    starting:
      model.workspace !== null &&
      Object.values(model.starts).some((request) => request.workspace === model.workspace),
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

type Draft = { -readonly [K in keyof SessionModel]: SessionModel[K] } & {
  effects: SessionEffect[];
};

function draft(model: SessionModel): Draft {
  // Only these fresh containers are mutable; nested requests and payloads stay shared.
  return {
    ...model,
    connection: { ...model.connection },
    workspacePrompt: { ...model.workspacePrompt },
    starts: { ...model.starts },
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

function sameScope(left: WorkspaceState | null, right: WorkspaceState | null): boolean {
  return JSON.stringify(left?.scope.include ?? []) === JSON.stringify(right?.scope.include ?? []);
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
  const workspace = draft.workspace;
  if (draft.connection.state === null || !workspace || draft.history.request) {
    return;
  }
  const id = nextId(draft);
  draft.history.request = { id, status: "current" };
  draft.history.error = null;
  draft.effects.push({ type: "listInvocations", id, workspace });
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
  const workspace = draft.workspace;
  const state = wsEntry(draft.connection.state, workspace);
  if (!workspace || !state || state.snapshotFetchedAt === null) return;
  if (draft.targets.snapshot.requestId !== null) return;
  const id = nextId(draft);
  draft.targets.snapshot.requestId = id;
  draft.targets.snapshot.error = null;
  draft.effects.push({ type: "getSnapshot", id, workspace });
}

function targetReadArgs(
  draft: Draft,
): { workspace: string; label: string; fetchedAt: number } | null {
  const selection = draft.targets.selection;
  const workspace = draft.workspace;
  const snapshot = draft.targets.snapshot.value;
  const state = wsEntry(draft.connection.state, workspace);
  if (
    !workspace ||
    !state ||
    !draft.connection.state?.bazel ||
    selection?.kind !== "target" ||
    !snapshot ||
    !snapshot.targets.some((target) => target.label === selection.label) ||
    snapshot.meta.fetchedAt !== state.snapshotFetchedAt
  )
    return null;
  return { workspace, label: selection.label, fetchedAt: snapshot.meta.fetchedAt };
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
  const workspace = draft.workspace;
  if (!invocationId || !workspace) return;
  const id = draft.saved.detail.wanted;
  draft.saved.detail.running = id;
  draft.saved.detail.error = null;
  draft.effects.push({ type: "getInvocation", id, workspace, invocationId });
}

function startProfile(draft: Draft) {
  if (
    draft.saved.profile.running !== null ||
    draft.saved.profile.wanted === null ||
    !savedEligible(draft)
  )
    return;
  const invocationId = selectedInvocation(draft);
  const workspace = draft.workspace;
  if (!invocationId || !workspace) return;
  const id = draft.saved.profile.wanted;
  draft.saved.profile.running = id;
  draft.saved.profile.error = null;
  draft.effects.push({ type: "getProfile", id, workspace, invocationId });
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
  if (left.kind === "console" && right.kind === "console") {
    return left.id === right.id && left.page === right.page;
  }
  return true;
}

function showContent(draft: Draft, content: SessionContent) {
  if (sameContent(draft.content, content)) return;
  draft.navigation++;
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
  draft.navigation++;
  draft.targets.selection = selection;
  resetTargetContext(draft);
}

function selectWorkspace(draft: Draft, workspace: string) {
  if (draft.workspace === workspace) return;
  const page = workspacePage(draft);
  draft.workspace = workspace;
  draft.navigation++;
  draft.content = { kind: page };
  draft.targets.selection = null;
  draft.targets.snapshot.requestId = null;
  draft.targets.snapshot.value = null;
  draft.targets.snapshot.error = null;
  draft.targets.detail = emptyReader<TargetDetail>();
  draft.targets.outputs = emptyReader<TargetOutputs>();
  draft.targets.actions = emptyReader<TargetActions>();
  draft.history.records = [];
  draft.history.error = null;
  if (draft.history.request) draft.history.request.status = "invalidated";
  // The saved readers' in-flight workers are global; keep them so their
  // replies still release the slot, but drop everything else for the new
  // workspace's selection.
  draft.saved.detail = {
    ...emptyReader<InvocationDetail>(),
    running: draft.saved.detail.running,
  };
  draft.saved.profile = {
    ...emptyReader<Section<Profile>>(),
    running: draft.saved.profile.running,
  };
  draft.connection.error = null;
  startHistory(draft);
  startSnapshot(draft);
  refreshMissingTargets(draft);
}

function refreshMissingTargets(draft: Draft) {
  const state = wsEntry(draft.connection.state, draft.workspace);
  if (
    state &&
    draft.connection.state?.bazel &&
    state.snapshotFetchedAt === null &&
    state.ingest === "idle" &&
    state.lastError === null
  ) {
    draft.effects.push({ type: "refresh", workspace: state.workspace });
  }
}

function updateTargetsForWorkspace(
  draft: Draft,
  before: WorkspaceState | null,
  after: WorkspaceState | null,
) {
  const scopeChanged = !sameScope(before, after);
  const fetchedChanged = (before?.snapshotFetchedAt ?? null) !== (after?.snapshotFetchedAt ?? null);
  const contextChanged = before === null || scopeChanged || fetchedChanged || after === null;
  let selectionCleared = false;
  if (after === null || scopeChanged || after.snapshotFetchedAt === null) {
    selectionCleared = draft.targets.selection !== null;
    draft.targets.selection = null;
    draft.targets.snapshot.value = null;
    draft.targets.snapshot.error = null;
  }
  if (contextChanged || selectionCleared) resetTargetContext(draft);
  if (!contextChanged) {
    if (draft.targets.snapshot.error !== null) startSnapshot(draft);
    return;
  }
  draft.targets.snapshot.requestId = null;
  if (after === null || scopeChanged || after.snapshotFetchedAt === null) {
    draft.targets.snapshot.value = null;
  }
  startSnapshot(draft);
  if (scopeChanged) refreshMissingTargets(draft);
}

function applyAppState(draft: Draft, state: AppState) {
  const before = draft.connection.state;
  if (before !== null && state.revision <= before.revision) return;
  const wasSavedEligible = savedEligible(draft);
  const workspaceBefore = draft.workspace;
  draft.connection.state = state;
  if (before === null) draft.connection.error = null;

  if (
    draft.workspace === null &&
    state.lastWorkspace !== null &&
    state.workspaces.some((entry) => entry.workspace === state.lastWorkspace)
  ) {
    selectWorkspace(draft, state.lastWorkspace);
  }

  // A workspace selected above already reset its readers; only an unchanged
  // selection needs its target context reconciled against the new snapshot.
  if (draft.workspace !== null && draft.workspace === workspaceBefore) {
    updateTargetsForWorkspace(
      draft,
      wsEntry(before, draft.workspace),
      wsEntry(state, draft.workspace),
    );
  }
  savedEligibilityChanged(draft, wasSavedEligible, savedEligible(draft));
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
  const workspace = draft.workspace;
  if (!workspace || !canSend(sessionView(draft), pattern)) return;
  const id = nextId(draft);
  draft.starts[id] = {
    workspace,
    navigation: draft.navigation,
  };
  draft.connection.error = null;
  draft.effects.push({
    type: "startInvocation",
    id,
    workspace,
    cmd,
    pattern,
    argumentsText,
  });
}

function startResult(draft: Draft, requestId: number, result: Result<OwnedInvocation>) {
  const pending = draft.starts[requestId];
  if (!pending) return;
  delete draft.starts[requestId];
  if (!result.ok) {
    if (pending.workspace === draft.workspace && pending.navigation === draft.navigation) {
      draft.connection.error = result.error;
    }
    return;
  }
  if (pending.workspace === draft.workspace && pending.navigation === draft.navigation) {
    showContent(draft, { kind: "console", id: result.value.id, page: workspacePage(draft) });
  }
}

function stateResult(draft: Draft, result: Result<AppState>) {
  if (result.ok) {
    applyAppState(draft, result.value);
  } else if (draft.connection.state === null) {
    draft.connection.error = `Could not connect: ${result.error}`;
  }
}

function openConsole(draft: Draft, id: string) {
  const owned = ownedEntry(draft.connection.state, id);
  if (owned?.console) {
    if (draft.content.kind === "console" && draft.content.id === id) return;
    showContent(draft, { kind: "console", id, page: workspacePage(draft) });
    return;
  }
  selectSaved(draft, id);
}

function setWorkspacePromptOpen(draft: Draft, open: boolean) {
  if (draft.workspacePrompt.status === "opening") return;
  draft.workspacePrompt.status = open ? "editing" : "closed";
  draft.workspacePrompt.error = null;
}

function requestWorkspaceOpen(draft: Draft, path: string, scope: TargetScope) {
  draft.navigation++;
  draft.workspacePrompt.status = "opening";
  draft.workspacePrompt.error = null;
  const id = nextId(draft);
  draft.workspacePrompt.requestId = id;
  draft.effects.push({ type: "openWorkspace", id, path, scope });
}

function finishWorkspaceOpen(draft: Draft, requestId: number, result: Result<OpenWorkspaceResult>) {
  // The embedded aggregate is revision-guarded, so even a stale reply may
  // apply; only the latest request owns the prompt and the selection.
  if (result.ok) applyAppState(draft, result.value.state);
  if (draft.workspacePrompt.requestId !== requestId) return;
  draft.workspacePrompt.requestId = null;
  draft.workspacePrompt.status = result.ok ? "closed" : "editing";
  draft.workspacePrompt.error = result.ok ? null : result.error;
  if (result.ok) selectWorkspace(draft, result.value.workspace);
  resumeTargets(draft);
}

function requestStop(draft: Draft, id: string) {
  const execution = ownedEntry(draft.connection.state, id);
  if (!execution?.stoppable || execution.stopping) return;
  draft.effects.push({ type: "stop", id: execution.id, workspace: execution.workspace });
}

function requestRefresh(draft: Draft) {
  const workspace = draft.workspace;
  if (!workspace || draft.connection.state === null) return;
  draft.effects.push({ type: "refresh", workspace });
}

export function update(model: SessionModel, message: SessionMessage): SessionTransition {
  const next = draft(model);
  switch (message.type) {
    case "subscriptionsFailed":
      next.connection.error = `Could not connect: ${message.error}`;
      break;
    case "stateResult":
      stateResult(next, message.result);
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
      openConsole(next, message.id);
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
    case "workspacePrompt":
      setWorkspacePromptOpen(next, message.open);
      break;
    case "openWorkspace":
      requestWorkspaceOpen(next, message.path, message.scope);
      break;
    case "workspaceResult":
      finishWorkspaceOpen(next, message.requestId, message.result);
      break;
    case "stop":
      requestStop(next, message.id);
      break;
    case "stopResult":
      if (!message.result.ok && contentInvocationId(next) === message.id) {
        next.connection.error = message.result.error;
      }
      break;
    case "refresh":
      requestRefresh(next);
      break;
    case "refreshResult":
      if (!message.result.ok && message.workspace === next.workspace) {
        next.connection.error = message.result.error;
      }
      break;
    default:
      unexpected(message);
  }
  return finishDraft(next);
}
