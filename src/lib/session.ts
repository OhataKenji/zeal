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
  TargetOutputs,
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
  /** Configuration-resolved output paths from `bazel cquery`. */
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
    targetOutputs: null,
    targetOutputsError: null,
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

/** Index/detail phase markers emitted as binary "idle" | "running" flags. */
const RUNNING_SLOTS = ["ingest", "detailQuery"] as const;

/** Whether a serialized Bazel operation occupies the workspace. Matches the core's bazel_busy. */
function bazelBusy(state: AppState | null): boolean {
  return bazelBusyReason(state) !== null;
}

export function bazelBusyReason(state: AppState | null): string | null {
  if (state?.activeInvocation) return "a Bazel operation is in progress";
  const reasons = {
    ingest: "targets are loading",
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

/** Returns whether the core can stop the active Bazel operation. */
export function canStop(view: SessionView): boolean {
  return !!(view.ready && view.state?.activeInvocation && !view.state.stopping);
}

type SliceKey = readonly unknown[];
type TargetKey = readonly [
  string | null,
  string,
  { kind: "target" | "package"; label: string } | null,
  number | null,
  number | null,
];
type SliceStatus = {
  loading?: "snapshotLoading" | "detailLoading" | "profileLoading";
  error?:
    | "invocationsError"
    | "snapshotError"
    | "targetDetailError"
    | "targetOutputsError"
    | "detailError"
    | "profileError";
  formatError?: (error: unknown) => string;
};
type Slice<K extends SliceKey, T> = {
  key: () => K;
  enabled: () => boolean;
  fetch: (key: K) => Promise<T>;
  success: (value: T, key: K) => Partial<SessionView>;
  onSuccess?: (key: K) => void;
  failure?: (error: unknown) => Partial<SessionView>;
  reset: (previous: K | null, next: K) => Partial<SessionView>;
  status?: SliceStatus;
  mode?: "coalesce" | "overlap";
  invalidateOnChange?: (previous: K | null, next: K) => boolean;
  acceptStale?: (request: K, current: K) => boolean;
};
type SliceRunner = {
  state: {
    key: SliceKey | null;
    attempted: string | null;
    inFlight: Map<string, Promise<void>>;
    generation: number;
  };
  key: () => SliceKey;
  enabled: () => boolean;
  load: (key: SliceKey) => Promise<Partial<SessionView>>;
  onSuccess?: (key: SliceKey) => void;
  failure: (error: unknown) => Partial<SessionView>;
  reset: (previous: SliceKey | null, next: SliceKey) => Partial<SessionView>;
  status?: SliceStatus;
  mode?: "coalesce" | "overlap";
  invalidateOnChange?: (previous: SliceKey | null, next: SliceKey) => boolean;
  acceptStale?: (request: SliceKey, current: SliceKey) => boolean;
};

/**
 * Runs one workspace session. Slice dependencies and lifecycle state are
 * private so the public view and API remain stable.
 */
export function createSession(transport: Transport, changed: (view: SessionView) => void) {
  let view = emptyView();
  let disposed = false;
  let stateRevision = 0;
  let startToken = 0;
  let syncing = false;
  let syncAgain = false;
  let slices: SliceRunner[] = [];
  let snapshotSlice: SliceRunner;
  let invocationSlice: SliceRunner;
  let targetDetailSlice: SliceRunner;
  let targetDetailLoadedKey: string | null = null;
  let unlisten: (() => void) | undefined;
  let unlistenHistory: (() => void) | undefined;

  type Lifecycle =
    | { kind: "idle" }
    | {
        kind: "starting";
        workspace: string;
        cmd: BazelCommand;
        pattern: string;
        previous: Lifecycle;
        observedId?: string;
      }
    | { kind: "running"; workspace: string; id: string; observed: boolean }
    | { kind: "completed"; workspace: string; id: string };

  let lifecycle: Lifecycle = { kind: "idle" };

  function update(patch: Partial<SessionView>, sync = true) {
    if (disposed || Object.keys(patch).length === 0) return;
    view = { ...view, ...patch };
    changed(view);
    if (sync) syncSlices();
  }

  function createSlice<K extends SliceKey, T>(definition: Slice<K, T>): SliceRunner {
    return {
      state: { key: null, attempted: null, inFlight: new Map(), generation: 0 },
      key: definition.key,
      enabled: definition.enabled,
      load: async (key) => {
        const typedKey = key as K;
        return definition.success(await definition.fetch(typedKey), typedKey);
      },
      onSuccess: definition.onSuccess ? (key) => definition.onSuccess?.(key as K) : undefined,
      failure: (error) => definition.failure?.(error) ?? {},
      reset: (previous, next) => definition.reset(previous as K | null, next as K),
      status: definition.status,
      mode: definition.mode,
      invalidateOnChange: definition.invalidateOnChange
        ? (previous, next) =>
            definition.invalidateOnChange?.(previous as K | null, next as K) ?? true
        : undefined,
      acceptStale: definition.acceptStale
        ? (request, current) => definition.acceptStale?.(request as K, current as K) ?? false
        : undefined,
    };
  }

  function identity(key: SliceKey): string {
    return JSON.stringify(key);
  }

  function accepts(slice: SliceRunner, key: SliceKey, token: string, generation: number): boolean {
    const currentKey = slice.state.key ?? [];
    return (
      generation === slice.state.generation &&
      (token === identity(currentKey) || slice.acceptStale?.(key, currentKey) === true)
    );
  }

  function statusPatch(
    slice: SliceRunner,
    loading: boolean,
    error: unknown = null,
    clearError = true,
  ): Partial<SessionView> {
    const status = slice.status;
    if (!status) return {};
    const patch: Partial<SessionView> = {};
    if (status.loading) Object.assign(patch, { [status.loading]: loading });
    if (status.error) {
      Object.assign(patch, {
        [status.error]: clearError ? null : (status.formatError?.(error) ?? String(error)),
      });
    }
    return patch;
  }

  function updateSliceKeys(authoritative: boolean): Partial<SessionView> {
    const reset: Partial<SessionView> = {};
    for (const slice of slices) {
      const key = slice.key();
      const previous = slice.state.key;
      if (previous === null || identity(previous) !== identity(key)) {
        if (slice.invalidateOnChange?.(previous, key) ?? true) slice.state.generation++;
        slice.state.key = key;
        slice.state.attempted = null;
        Object.assign(reset, slice.reset(previous, key), statusPatch(slice, false));
      } else if (authoritative && slice === snapshotSlice && view.snapshotError !== null) {
        slice.state.attempted = null;
      }
    }
    return reset;
  }

  async function loadSection(slice: SliceRunner, key: SliceKey, token: string, generation: number) {
    try {
      update(statusPatch(slice, true), false);
      const patch = await slice.load(key);
      if (!disposed && accepts(slice, key, token, generation)) {
        slice.onSuccess?.(key);
        update({ ...statusPatch(slice, false), ...patch }, false);
      }
    } catch (error) {
      if (!disposed && accepts(slice, key, token, generation)) {
        update(
          {
            ...statusPatch(slice, false, error, false),
            ...slice.failure(error),
          },
          false,
        );
      }
    } finally {
      slice.state.inFlight.delete(token);
      syncSlices();
    }
  }

  function startSlice(slice: SliceRunner) {
    if (disposed || !slice.enabled()) return;
    const key = slice.state.key ?? slice.key();
    const token = identity(key);
    if (slice.state.attempted === token || slice.state.inFlight.has(token)) return;
    if (slice.mode !== "overlap" && slice.state.inFlight.size > 0) return;
    slice.state.attempted = token;
    const generation = slice.state.generation;
    let resolve!: () => void;
    const request = new Promise<void>((done) => {
      resolve = done;
    });
    slice.state.inFlight.set(token, request);
    void loadSection(slice, key, token, generation).then(resolve, resolve);
  }

  function forceSlice(slice: SliceRunner): Promise<void> {
    if (disposed || !slice.enabled()) return Promise.resolve();
    const key = slice.key();
    const token = identity(key);
    const pending = slice.state.inFlight.get(token);
    if (pending) {
      if (slice.mode === "coalesce") slice.state.attempted = null;
      return pending;
    }
    if (slice.mode === "coalesce" && slice.state.inFlight.size > 0) {
      slice.state.attempted = null;
      return slice.state.inFlight.values().next().value ?? Promise.resolve();
    }
    slice.state.attempted = null;
    syncSlices();
    return slice.state.inFlight.get(token) ?? Promise.resolve();
  }

  function syncPass(authoritative: boolean) {
    const reset = updateSliceKeys(authoritative);
    if (Object.keys(reset).length > 0) update(reset, false);
    if (!syncAgain) slices.forEach(startSlice);
  }

  function syncSlices(authoritative = false) {
    if (disposed) return;
    if (syncing) {
      syncAgain = true;
      return;
    }
    syncing = true;
    try {
      do {
        syncAgain = false;
        syncPass(authoritative);
      } while (syncAgain);
    } finally {
      syncing = false;
      if (syncAgain) syncSlices();
    }
  }

  function scopeKey(state: AppState | null): string {
    return JSON.stringify(state?.scope.include ?? []);
  }

  function selectedRecord(): InvocationRecord | undefined {
    return view.invocations.find((invocation) => invocation.id === view.selectedInvocation);
  }

  function targetKey(): TargetKey {
    const selection = view.selection;
    return [
      view.state?.workspace ?? null,
      scopeKey(view.state),
      selection?.kind === "target"
        ? { kind: "target", label: selection.label }
        : selection?.kind === "package"
          ? { kind: "package", label: selection.path }
          : null,
      view.state?.snapshotFetchedAt ?? null,
      view.snapshot?.meta.fetchedAt ?? null,
    ] as const;
  }

  function targetReady(): boolean {
    const state = view.state;
    const snapshot = view.snapshot;
    const selection = view.selection;
    return !!(
      selection?.kind === "target" &&
      snapshot &&
      snapshot.targets.some((target) => target.label === selection.label) &&
      snapshot.meta.fetchedAt === state?.snapshotFetchedAt &&
      workspaceReady(view) &&
      !sessionBusy(view)
    );
  }

  invocationSlice = createSlice({
    key: () =>
      [view.state?.workspace ?? null, view.state?.activeInvocation ?? null, view.ready] as const,
    enabled: () => !!(view.state?.workspace && view.ready),
    fetch: () => transport.invoke<InvocationRecord[]>(COMMANDS.listInvocations),
    mode: "coalesce",
    invalidateOnChange: (previous, next) => !previous || previous[0] !== next[0],
    acceptStale: (request, current) => request[0] === current[0],
    status: { error: "invocationsError" },
    success: (invocations) => {
      if (
        JSON.stringify(invocations) === JSON.stringify(view.invocations) &&
        view.invocationsError === null
      ) {
        return {};
      }
      return { invocations };
    },
    reset: (previous, next) => (previous && previous[0] !== next[0] ? { invocations: [] } : {}),
  });

  snapshotSlice = createSlice({
    key: () =>
      [
        view.state?.workspace ?? null,
        scopeKey(view.state),
        view.state?.snapshotFetchedAt ?? null,
      ] as const,
    enabled: () => !!(view.state?.workspace && view.state.snapshotFetchedAt !== null),
    fetch: () => transport.invoke<TargetIndex | null>(COMMANDS.getSnapshot),
    mode: "overlap",
    status: {
      loading: "snapshotLoading",
      error: "snapshotError",
      formatError: (error) => `Could not load the workspace snapshot: ${String(error)}`,
    },
    success: (value) => {
      const selection = view.selection;
      const selectionIsValid =
        selection?.kind === "target" &&
        value?.targets.some((target) => target.label === selection.label);
      return {
        snapshot: value,
        selection:
          selection?.kind === "package" || (selection?.kind === "target" && !selectionIsValid)
            ? null
            : selection,
        ...(view.targetDetailError === "Could not load the target list"
          ? { targetDetailError: null }
          : {}),
      };
    },
    failure: () =>
      view.selection?.kind === "target"
        ? { targetDetailError: "Could not load the target list" }
        : {},
    reset: (previous, next) =>
      previous && (previous[0] !== next[0] || previous[1] !== next[1] || next[2] === null)
        ? { snapshot: null }
        : {},
  });

  targetDetailSlice = createSlice({
    key: targetKey,
    enabled: targetReady,
    fetch: async (key) => {
      const selection = key[2];
      if (selection?.kind !== "target" || key[3] === null) {
        throw new Error("Target detail requires a selected target and snapshot");
      }
      const detail = await transport.invoke<TargetDetail>(COMMANDS.getTargetDetail, {
        label: selection.label,
        fetchedAt: key[3],
      });
      if (detail.target.label !== selection.label) {
        throw new Error("Target detail does not match the selected label");
      }
      return detail;
    },
    status: { error: "targetDetailError" },
    success: (value) => ({ targetDetail: value }),
    onSuccess: (key) => {
      targetDetailLoadedKey = identity(key);
    },
    reset: () => {
      targetDetailLoadedKey = null;
      return { targetDetail: null };
    },
  });

  const targetOutputsSlice = createSlice({
    key: () => [...targetKey(), targetDetailLoadedKey] as const,
    enabled: () =>
      targetReady() &&
      view.targetDetail !== null &&
      targetDetailLoadedKey === identity(targetKey()),
    fetch: (key) => {
      const selection = key[2];
      if (selection?.kind !== "target" || key[3] === null) {
        throw new Error("Target outputs require a selected target and snapshot");
      }
      return transport.invoke<TargetOutputs>(COMMANDS.getTargetOutputs, {
        label: selection.label,
        fetchedAt: key[3],
      });
    },
    status: { error: "targetOutputsError" },
    success: (value) => ({ targetOutputs: value }),
    reset: () => ({ targetOutputs: null }),
  });

  function detailKey() {
    const id = view.selectedInvocation;
    const eligible = !!(id && view.ready && !view.starting && id !== view.state?.activeInvocation);
    return [
      view.state?.workspace ?? null,
      id,
      eligible,
      JSON.stringify(selectedRecord() ?? null),
    ] as const;
  }

  function resetInvocationData(previous: SliceKey | null, next: SliceKey) {
    return previous && (previous[0] !== next[0] || previous[1] !== next[1]) ? { detail: null } : {};
  }

  const detailSlice = createSlice({
    key: detailKey,
    enabled: () => !!detailKey()[0] && !!detailKey()[1] && detailKey()[2] === true,
    fetch: (key) => transport.invoke<InvocationDetail>(COMMANDS.getInvocation, { id: key[1] }),
    status: { loading: "detailLoading", error: "detailError" },
    success: (value) => ({ detail: value }),
    reset: resetInvocationData,
  });

  const profileSlice = createSlice({
    key: detailKey,
    enabled: () => !!detailKey()[0] && !!detailKey()[1] && detailKey()[2] === true,
    fetch: (key) =>
      transport.invoke<Section<Profile>>(COMMANDS.getInvocationProfile, { id: key[1] }),
    status: { loading: "profileLoading", error: "profileError" },
    success: (value) => ({ profile: value }),
    reset: (previous, next) =>
      previous && (previous[0] !== next[0] || previous[1] !== next[1]) ? { profile: null } : {},
  });

  slices = [
    invocationSlice,
    snapshotSlice,
    targetDetailSlice,
    targetOutputsSlice,
    detailSlice,
    profileSlice,
  ];

  function loadInvocations(): Promise<void> {
    return forceSlice(invocationSlice);
  }

  function loadSnapshot(): Promise<void> {
    return forceSlice(snapshotSlice);
  }

  function loadDetail(): Promise<void> {
    return forceSlice(detailSlice);
  }

  function loadProfile(): Promise<void> {
    return forceSlice(profileSlice);
  }

  function sameSelection(left: ExplorerSelection, right: ExplorerSelection): boolean {
    if (left?.kind !== right?.kind) return false;
    if (left?.kind === "target" && right?.kind === "target") return left.label === right.label;
    if (left?.kind === "package" && right?.kind === "package") return left.path === right.path;
    return left === null && right === null;
  }

  function select(selection: ExplorerSelection) {
    if (sameSelection(selection, view.selection)) {
      if (selection?.kind === "target" && view.targetDetailError) {
        void forceSlice(targetDetailSlice);
      }
      return;
    }
    update({ selection });
  }

  function selectInvocation(id: string | null) {
    const changedId = id !== view.selectedInvocation;
    update({ consoleOpen: false, selectedInvocation: id });
    if (!changedId) {
      void loadDetail();
      void loadProfile();
    }
  }

  function openConsole(record: InvocationRecord) {
    if (record.id !== (view.state?.consoleInvocation ?? view.state?.activeInvocation)) {
      selectInvocation(record.id);
      return;
    }
    if (view.consoleOpen && view.console?.id === record.id) return;
    update({ console: record, consoleOpen: true, selectedInvocation: null });
  }

  function completeConsole(id: string) {
    if (
      view.starting ||
      view.state?.activeInvocation ||
      !view.consoleOpen ||
      view.console?.id !== id
    ) {
      return;
    }
    selectInvocation(id);
  }

  function transitionStarting(
    current: Extract<Lifecycle, { kind: "starting" }>,
    state: AppState,
  ): null {
    if (current.workspace !== state.workspace) {
      lifecycle = { kind: "idle" };
      return null;
    }
    const activeId = state.activeInvocation;
    if (!activeId || state.consoleInvocation !== activeId) return null;
    const shouldOpen = current.observedId !== activeId;
    lifecycle = { ...current, observedId: activeId };
    if (shouldOpen) {
      update(
        {
          console: { id: activeId, cmd: current.cmd, pattern: current.pattern },
          consoleOpen: true,
          selectedInvocation: null,
        },
        false,
      );
    }
    return null;
  }

  function transitionRunning(
    current: Extract<Lifecycle, { kind: "running" }>,
    state: AppState,
    before: AppState | null,
  ): string | null {
    const activeId = state.activeInvocation;
    if (activeId === current.id) {
      lifecycle = { ...current, observed: true };
      return null;
    }
    if (activeId) {
      const completed =
        current.observed && before?.activeInvocation === current.id ? current.id : null;
      lifecycle = {
        kind: "running",
        workspace: state.workspace ?? current.workspace,
        id: activeId,
        observed: true,
      };
      return completed;
    }

    const completed =
      (current.observed && before?.activeInvocation === current.id) ||
      state.consoleInvocation === current.id;
    if (completed) {
      lifecycle = { kind: "completed", workspace: current.workspace, id: current.id };
      return current.id;
    }
    lifecycle = { kind: "idle" };
    return null;
  }

  function transitionAppState(
    state: AppState,
    before: AppState | null,
    workspaceChanged: boolean,
  ): string | null {
    if (workspaceChanged) {
      lifecycle = state.activeInvocation
        ? {
            kind: "running",
            workspace: state.workspace ?? "",
            id: state.activeInvocation,
            observed: true,
          }
        : { kind: "idle" };
      return null;
    }
    if (lifecycle.kind === "starting") return transitionStarting(lifecycle, state);
    if (lifecycle.kind === "running") return transitionRunning(lifecycle, state, before);
    if (state.activeInvocation) {
      lifecycle = {
        kind: "running",
        workspace: state.workspace ?? "",
        id: state.activeInvocation,
        observed: true,
      };
    }
    return null;
  }

  function acceptState(state: AppState) {
    if (disposed) return;
    const before = view.state;
    const resumed = !view.ready;
    const workspaceChanged = state.workspace !== before?.workspace;
    const scopeChanged =
      JSON.stringify(state.scope.include) !== JSON.stringify(before?.scope.include);
    ++stateRevision;
    const patch: Partial<SessionView> = {
      state,
      ready: true,
      ...(resumed ? { error: null } : {}),
    };
    if (workspaceChanged) {
      ++startToken;
      Object.assign(patch, {
        console: null,
        consoleOpen: false,
        selectedInvocation: null,
        selection: null,
        starting: false,
      });
    } else if (scopeChanged || state.snapshotFetchedAt === null) {
      patch.selection = null;
    }
    update(patch, false);
    const completed = transitionAppState(state, before, workspaceChanged);
    if (completed) completeConsole(completed);
    syncSlices(true);
  }

  function isCurrentStart(token: number, workspace: string): boolean {
    return !disposed && token === startToken && workspace === view.state?.workspace;
  }

  function finishStart(workspace: string, token: number, record: InvocationRecord | null) {
    if (!isCurrentStart(token, workspace)) return;
    if (lifecycle.kind !== "starting" || lifecycle.workspace !== workspace) {
      update({ starting: false });
      return;
    }
    const current = lifecycle;
    let completed: string | null = null;
    if (!record) {
      lifecycle = current.previous;
    } else if (!view.ready) {
      lifecycle = {
        kind: "running",
        workspace,
        id: record.id,
        observed: current.observedId === record.id,
      };
    } else if (view.state?.activeInvocation === record.id) {
      lifecycle = { kind: "running", workspace, id: record.id, observed: true };
    } else if (!view.state?.activeInvocation) {
      lifecycle = { kind: "completed", workspace, id: record.id };
      completed = record.id;
    } else {
      lifecycle = {
        kind: "running",
        workspace,
        id: view.state.activeInvocation,
        observed: true,
      };
    }
    update({ starting: false }, false);
    syncSlices();
    if (completed) completeConsole(completed);
  }

  function dispose() {
    disposed = true;
    for (const slice of slices) slice.state.generation++;
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
      if (!view.ready) update({ error: `Could not connect: ${String(error)}` });
    }
  }

  async function subscribe() {
    try {
      const removeHistory = await transport.listen<void>("invocations-changed", () => {
        void forceSlice(invocationSlice);
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

  async function reconcileStartedState(workspace: string, token: number) {
    const revision = stateRevision;
    try {
      const state = await transport.invoke<AppState>(COMMANDS.getState);
      if (!isCurrentStart(token, workspace)) return;
      if (revision === stateRevision) acceptState(state);
    } catch (error) {
      if (isCurrentStart(token, workspace) && revision === stateRevision) {
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
    const token = ++startToken;
    lifecycle = {
      kind: "starting",
      workspace,
      cmd,
      pattern,
      previous: lifecycle,
    };
    update({ starting: true, error: null });
    let record: InvocationRecord | null = null;
    try {
      record = await transport.invoke<InvocationRecord>(COMMANDS.startInvocation, {
        cmd,
        pattern,
        arguments: argumentsText,
      });
      if (!isCurrentStart(token, workspace)) return;
      if (!view.consoleOpen || view.console?.id !== record.id) {
        update({
          console: { ...record, cmd, pattern },
          consoleOpen: true,
          selectedInvocation: null,
        });
      }
      await reconcileStartedState(workspace, token);
      if (isCurrentStart(token, workspace)) void forceSlice(invocationSlice);
    } catch (error) {
      if (isCurrentStart(token, workspace)) update({ error: String(error) });
    } finally {
      finishStart(workspace, token, record);
    }
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
    }
  }

  async function refresh() {
    if (sessionBusy(view)) return;
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
