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

type RequestSlot = ReturnType<typeof createRequestSlot>;
type SliceKey = readonly unknown[];
type Slice = {
  slot: RequestSlot;
  inFlight: Set<string>;
  key: () => SliceKey;
  enabled: () => boolean;
  fetch: (key: SliceKey) => Promise<unknown>;
  pending?: () => Partial<SessionView>;
  success: (value: unknown, key: SliceKey, token: string) => Partial<SessionView>;
  failure: (error: unknown, key: SliceKey) => Partial<SessionView>;
  reset: (previous: SliceKey | null, next: SliceKey) => Partial<SessionView>;
  mode?: "coalesce" | "overlap";
  invalidateOnChange?: (previous: SliceKey | null, next: SliceKey) => boolean;
  acceptStale?: (request: SliceKey, current: SliceKey) => boolean;
  onTokenChange?: () => void;
  identity: string | null;
  token: string | null;
  currentKey: SliceKey | null;
  attempted: string | null;
  failed: string | null;
  revision: number;
  pendingAgain: boolean;
  lastPromise: Promise<void>;
};

/** In-flight state for one async view slice. */
function createRequestSlot() {
  let generation = 0;
  let requests = 0;
  return {
    claim() {
      requests++;
      return ++generation;
    },
    release() {
      requests--;
    },
    invalidate() {
      generation++;
    },
    get current() {
      return generation;
    },
    get busy() {
      return requests > 0;
    },
  };
}

/**
 * Runs one workspace session. Slice dependencies and lifecycle state are
 * private so the public view and API remain stable.
 */
export function createSession(transport: Transport, changed: (view: SessionView) => void) {
  let view = emptyView();
  let disposed = false;
  let stateRevision = 0;
  let syncing = false;
  let syncAgain = false;
  let slices: Slice[] = [];
  let snapshotSlice: Slice;
  let invocationSlice: Slice;
  let targetDetailSlice: Slice;
  let targetDetailValueToken: string | null = null;
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
        observedComplete?: boolean;
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

  function createSlice(
    definition: Omit<
      Slice,
      | "slot"
      | "inFlight"
      | "identity"
      | "token"
      | "currentKey"
      | "attempted"
      | "failed"
      | "revision"
      | "pendingAgain"
      | "lastPromise"
    >,
  ): Slice {
    return {
      ...definition,
      slot: createRequestSlot(),
      inFlight: new Set(),
      identity: null,
      token: null,
      currentKey: null,
      attempted: null,
      failed: null,
      revision: 0,
      pendingAgain: false,
      lastPromise: Promise.resolve(),
    };
  }

  function identity(key: SliceKey): string {
    return JSON.stringify(key);
  }

  function mergePatch(target: Partial<SessionView>, patch: Partial<SessionView>) {
    Object.assign(target, patch);
  }

  function updateSliceKeys(authoritative: boolean): Partial<SessionView> {
    const reset: Partial<SessionView> = {};
    for (const slice of slices) {
      const key = slice.key();
      const nextIdentity = identity(key);
      if (
        authoritative &&
        slice === snapshotSlice &&
        slice.identity === nextIdentity &&
        slice.failed === slice.token
      ) {
        slice.revision++;
      }
      if (slice.identity !== nextIdentity) {
        const previous = slice.currentKey;
        if (slice.invalidateOnChange?.(previous, key) ?? true) slice.slot.invalidate();
        slice.identity = nextIdentity;
        slice.currentKey = key;
        slice.attempted = null;
        slice.failed = null;
        mergePatch(reset, slice.reset(previous, key));
      } else {
        slice.currentKey = key;
      }
      const nextToken = JSON.stringify([nextIdentity, slice.revision]);
      if (slice.token !== nextToken) {
        slice.token = nextToken;
        slice.attempted = null;
        slice.failed = null;
        slice.onTokenChange?.();
      }
    }
    return reset;
  }

  async function loadSection(slice: Slice, key: SliceKey, token: string, generation: number) {
    try {
      if (slice.pending) update(slice.pending());
      const value = await slice.fetch(key);
      const currentKey = slice.currentKey ?? [];
      const accepted =
        generation === slice.slot.current &&
        (token === slice.token || slice.acceptStale?.(key, currentKey));
      if (!disposed && accepted) {
        update(slice.success(value, key, token));
      }
    } catch (error) {
      const currentKey = slice.currentKey ?? [];
      const accepted =
        generation === slice.slot.current &&
        (token === slice.token || slice.acceptStale?.(key, currentKey));
      if (!disposed && accepted) {
        slice.failed = token;
        update(slice.failure(error, key));
      }
    } finally {
      slice.inFlight.delete(token);
      slice.slot.release();
      const retry = slice.pendingAgain;
      slice.pendingAgain = false;
      if (retry) slice.attempted = null;
      if (generation !== slice.slot.current || retry) syncSlices();
    }
  }

  function startSlice(slice: Slice) {
    if (disposed || !slice.enabled() || !slice.token || !slice.currentKey) return;
    if (slice.attempted === slice.token) return;
    if (slice.slot.busy) {
      if (slice.mode === "coalesce") slice.pendingAgain = true;
      if (slice.mode === "overlap") {
        const key = slice.currentKey;
        const token = slice.token;
        slice.attempted = token;
        slice.failed = null;
        const generation = slice.slot.claim();
        slice.lastPromise = loadSection(slice, key, token, generation);
      }
      return;
    }
    const key = slice.currentKey;
    const token = slice.token;
    slice.attempted = token;
    slice.failed = null;
    const generation = slice.slot.claim();
    slice.inFlight.add(token);
    slice.lastPromise = loadSection(slice, key, token, generation);
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
        const reset = updateSliceKeys(authoritative);
        if (Object.keys(reset).length > 0) update(reset, false);
        if (syncAgain) continue;
        for (const slice of slices) startSlice(slice);
      } while (syncAgain);
    } finally {
      syncing = false;
      if (syncAgain) syncSlices();
    }
  }

  function forceSlice(slice: Slice, retry: Partial<SessionView> = {}): Promise<void> {
    if (disposed || !slice.enabled()) return Promise.resolve();
    if (slice.slot.busy) {
      if (slice.mode === "coalesce") slice.pendingAgain = true;
      if (slice.mode !== "overlap" || slice.inFlight.has(slice.token ?? "")) {
        return Promise.resolve();
      }
    }
    const previous = slice.lastPromise;
    if (Object.keys(retry).length > 0) update(retry, false);
    slice.revision++;
    slice.failed = null;
    syncSlices();
    return slice.lastPromise === previous ? Promise.resolve() : slice.lastPromise;
  }

  function scopeKey(state: AppState | null): string {
    return JSON.stringify(state?.scope.include ?? []);
  }

  function selectedRecord(): InvocationRecord | undefined {
    return view.invocations.find((invocation) => invocation.id === view.selectedInvocation);
  }

  function targetKey(): SliceKey {
    const selection = view.selection;
    return [
      view.state?.workspace ?? null,
      scopeKey(view.state),
      selection?.kind ?? null,
      selection?.kind === "target"
        ? selection.label
        : selection?.kind === "package"
          ? selection.path
          : null,
      view.state?.snapshotFetchedAt ?? null,
      view.snapshot?.meta.fetchedAt ?? null,
    ];
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
    key: () => [view.state?.workspace ?? null, view.state?.activeInvocation ?? null, view.ready],
    enabled: () => !!(view.state?.workspace && view.ready),
    fetch: () => transport.invoke<InvocationRecord[]>(COMMANDS.listInvocations),
    mode: "coalesce",
    invalidateOnChange: (previous, next) => !previous || previous[0] !== next[0],
    acceptStale: (request, current) => request[0] === current[0],
    pending: () => ({ invocationsError: null }),
    success: (value) => {
      const invocations = value as InvocationRecord[];
      if (
        JSON.stringify(invocations) === JSON.stringify(view.invocations) &&
        view.invocationsError === null
      ) {
        return {};
      }
      return { invocations, invocationsError: null };
    },
    failure: (error) => ({ invocationsError: String(error) }),
    reset: (previous, next) =>
      previous && previous[0] !== next[0] ? { invocations: [], invocationsError: null } : {},
  });

  snapshotSlice = createSlice({
    key: () => [
      view.state?.workspace ?? null,
      scopeKey(view.state),
      view.state?.snapshotFetchedAt ?? null,
    ],
    enabled: () => !!(view.state?.workspace && view.state.snapshotFetchedAt !== null),
    fetch: () => transport.invoke<TargetIndex | null>(COMMANDS.getSnapshot),
    mode: "overlap",
    pending: () => ({ snapshotLoading: true, snapshotError: null }),
    success: (value) => {
      const snapshot = value as TargetIndex | null;
      const selection = view.selection;
      const selectionIsValid =
        selection?.kind === "target" &&
        snapshot?.targets.some((target) => target.label === selection.label);
      if (
        view.targetDetailError === "Could not load the target list" &&
        view.targetDetail === null
      ) {
        targetDetailSlice.revision++;
      }
      return {
        snapshot,
        snapshotLoading: false,
        snapshotError: null,
        selection:
          selection?.kind === "package" || (selection?.kind === "target" && !selectionIsValid)
            ? null
            : selection,
        ...(view.targetDetailError === "Could not load the target list"
          ? { targetDetailError: null }
          : {}),
      };
    },
    failure: (error) => ({
      snapshotLoading: false,
      snapshotError: `Could not load the workspace snapshot: ${String(error)}`,
      ...(view.selection?.kind === "target"
        ? { targetDetailError: "Could not load the target list" }
        : {}),
    }),
    reset: (previous, next) =>
      previous && (previous[0] !== next[0] || previous[1] !== next[1] || next[2] === null)
        ? { snapshot: null, snapshotLoading: false, snapshotError: null }
        : {},
  });

  targetDetailSlice = createSlice({
    key: targetKey,
    enabled: targetReady,
    fetch: async (key) => {
      const label = key[2] === "target" ? (key[3] as string) : "";
      const fetchedAt = key[4] as number;
      const detail = await transport.invoke<TargetDetail>(COMMANDS.getTargetDetail, {
        label,
        fetchedAt,
      });
      if (detail.target.label !== label) {
        throw new Error("Target detail does not match the selected label");
      }
      return detail;
    },
    pending: () => ({ targetDetailError: null }),
    success: (value, _key, token) => {
      targetDetailValueToken = token;
      return { targetDetail: value as TargetDetail, targetDetailError: null };
    },
    failure: (error) => ({ targetDetailError: String(error) }),
    reset: () => {
      targetDetailValueToken = null;
      return {
        targetDetail: null,
        targetDetailError: null,
        targetOutputs: null,
        targetOutputsError: null,
      };
    },
    onTokenChange: () => {
      targetDetailValueToken = null;
    },
  });

  const targetOutputsSlice = createSlice({
    key: () => [...targetKey(), targetDetailSlice.revision],
    enabled: () =>
      targetReady() &&
      view.targetDetail !== null &&
      targetDetailValueToken === targetDetailSlice.token,
    fetch: (key) => {
      const label = key[2] === "target" ? (key[3] as string) : "";
      const fetchedAt = key[4] as number;
      return transport.invoke<TargetOutputs>(COMMANDS.getTargetOutputs, { label, fetchedAt });
    },
    pending: () => ({ targetOutputsError: null }),
    success: (value) => ({ targetOutputs: value as TargetOutputs, targetOutputsError: null }),
    failure: (error) => ({ targetOutputsError: String(error) }),
    reset: () => ({ targetOutputs: null, targetOutputsError: null }),
  });

  function detailKey(): SliceKey {
    const id = view.selectedInvocation;
    const eligible = !!(id && view.ready && !view.starting && id !== view.state?.activeInvocation);
    return [view.state?.workspace ?? null, id, eligible, JSON.stringify(selectedRecord() ?? null)];
  }

  function resetInvocationData(previous: SliceKey | null, next: SliceKey) {
    return previous && (previous[0] !== next[0] || previous[1] !== next[1])
      ? { detail: null, detailLoading: false, detailError: null }
      : { detailLoading: false, detailError: null };
  }

  const detailSlice = createSlice({
    key: detailKey,
    enabled: () => !!detailKey()[0] && !!detailKey()[1] && detailKey()[2] === true,
    fetch: (key) => transport.invoke<InvocationDetail>(COMMANDS.getInvocation, { id: key[1] }),
    pending: () => ({ detailLoading: true, detailError: null }),
    success: (value) => ({ detail: value as InvocationDetail, detailLoading: false }),
    failure: (error) => ({ detailError: String(error), detailLoading: false }),
    reset: resetInvocationData,
  });

  const profileSlice = createSlice({
    key: detailKey,
    enabled: () => !!detailKey()[0] && !!detailKey()[1] && detailKey()[2] === true,
    fetch: (key) =>
      transport.invoke<Section<Profile>>(COMMANDS.getInvocationProfile, { id: key[1] }),
    pending: () => ({ profileLoading: true, profileError: null }),
    success: (value) => ({ profile: value as Section<Profile>, profileLoading: false }),
    failure: (error) => ({ profileError: String(error), profileLoading: false }),
    reset: (previous, next) =>
      previous && (previous[0] !== next[0] || previous[1] !== next[1])
        ? { profile: null, profileLoading: false, profileError: null }
        : { profileLoading: false, profileError: null },
  });

  slices = [
    invocationSlice,
    snapshotSlice,
    targetDetailSlice,
    targetOutputsSlice,
    detailSlice,
    profileSlice,
  ];

  function refreshInvocations(): Promise<void> {
    return forceSlice(invocationSlice);
  }

  function loadInvocations(): Promise<void> {
    return refreshInvocations();
  }

  function loadSnapshot(): Promise<void> {
    if (snapshotSlice.inFlight.has(snapshotSlice.token ?? "")) {
      return Promise.resolve();
    }
    return forceSlice(snapshotSlice, { snapshotError: null });
  }

  function loadDetail(): Promise<void> {
    return forceSlice(detailSlice, { detailError: null });
  }

  function loadProfile(): Promise<void> {
    return forceSlice(profileSlice, { profileError: null });
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
        void forceSlice(targetDetailSlice, { targetDetailError: null });
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

  function transitionAppState(
    state: AppState,
    before: AppState | null,
    workspaceChanged: boolean,
    resumed: boolean,
  ): string | null {
    if (workspaceChanged) {
      lifecycle =
        resumed && state.activeInvocation
          ? {
              kind: "running",
              workspace: state.workspace ?? "",
              id: state.activeInvocation,
              observed: true,
            }
          : { kind: "idle" };
      return null;
    }
    const current = lifecycle;
    if (current.kind === "starting") {
      if (current.workspace !== state.workspace) {
        lifecycle = { kind: "idle" };
        return null;
      }
      const activeId = state.activeInvocation;
      if (activeId && state.consoleInvocation === activeId) {
        const shouldOpen = current.observedId !== activeId;
        lifecycle = { ...current, observedId: activeId, observedComplete: false };
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
      } else if (current.observedId && activeId !== current.observedId) {
        lifecycle = { ...current, observedComplete: true };
      }
      return null;
    }
    if (current.kind === "running") {
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
      if (current.observed && before?.activeInvocation === current.id) {
        lifecycle = { kind: "completed", workspace: current.workspace, id: current.id };
        return current.id;
      }
      if (state.consoleInvocation === current.id) {
        lifecycle = { kind: "completed", workspace: current.workspace, id: current.id };
        return current.id;
      }
      lifecycle = { kind: "idle" };
      return null;
    }
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
    const completed = transitionAppState(state, before, workspaceChanged, resumed);
    if (completed) completeConsole(completed);
    syncSlices(true);
  }

  function finishStart(workspace: string, record: InvocationRecord | null) {
    if (disposed || workspace !== view.state?.workspace) return;
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
    for (const slice of slices) slice.slot.invalidate();
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
        void refreshInvocations();
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
      if (disposed || workspace !== view.state?.workspace) return;
      if (!view.consoleOpen || view.console?.id !== record.id) {
        update({
          console: { ...record, cmd, pattern },
          consoleOpen: true,
          selectedInvocation: null,
        });
      }
      await reconcileStartedState(workspace);
      if (!disposed && workspace === view.state?.workspace) void refreshInvocations();
    } catch (error) {
      if (workspace === view.state?.workspace) update({ error: String(error) });
    } finally {
      finishStart(workspace, record);
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
