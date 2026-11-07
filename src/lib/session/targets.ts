import { COMMANDS, type Transport } from "../commands.ts";
import type { ExplorerSelection, TargetDetail, TargetIndex } from "../types.ts";

export interface TargetsView {
  snapshot: TargetIndex | null;
  snapshotLoading: boolean;
  snapshotError: string | null;
  selection: ExplorerSelection;
  targetDetail: TargetDetail | null;
  targetDetailError: string | null;
}

export interface TargetsContext {
  workspace: string | null;
  scopeKey: string;
  snapshotFetchedAt: number | null;
  canReadDetail: boolean;
}

export function emptyTargets(): TargetsView {
  return {
    snapshot: null,
    snapshotLoading: false,
    snapshotError: null,
    selection: null,
    targetDetail: null,
    targetDetailError: null,
  };
}

export function createTargets(
  transport: Pick<Transport, "invoke">,
  changed: (view: TargetsView) => void,
): {
  sync(context: TargetsContext): void;
  select(selection: ExplorerSelection): void;
  loadSnapshot(): Promise<void>;
  requestDetail(): void;
  dispose(): void;
} {
  let view = emptyTargets();
  let context: TargetsContext = {
    workspace: null,
    scopeKey: "[]",
    snapshotFetchedAt: null,
    canReadDetail: false,
  };
  let disposed = false;
  let snapshotGeneration = 0;
  let snapshotRequested: number | null = null;
  let targetGeneration = 0;
  let targetRequestInFlight = false;

  function update(patch: Partial<TargetsView>) {
    if (disposed) return;
    view = { ...view, ...patch };
    changed(view);
  }

  // Clearing a slice invalidates its in-flight loads, so a late response
  // cannot repopulate cleared data.
  function clearTargetDetail() {
    ++targetGeneration;
    return { targetDetail: null, targetDetailError: null };
  }
  function clearTargets() {
    ++snapshotGeneration;
    snapshotRequested = null;
    return {
      snapshot: null,
      snapshotLoading: false,
      snapshotError: null,
      selection: null,
      ...clearTargetDetail(),
    };
  }

  // Defensive: the core drops its snapshot only on a workspace switch,
  // which the reset above already clears.
  function clearSnapshot() {
    if (view.snapshot || view.snapshotError || snapshotRequested) {
      snapshotRequested = null;
      ++snapshotGeneration;
      update({ snapshot: null, snapshotLoading: false, snapshotError: null });
    }
  }

  // The DTO carries only the snapshot's fetchedAt; its data is fetched when
  // that timestamp changes, not on every state event.
  function acceptLoadedSnapshot(snapshot: TargetIndex | null) {
    snapshotRequested = null;
    update({ snapshot, snapshotLoading: false, snapshotError: null, targetDetailError: null });
    const selection = view.selection;
    if (
      selection?.kind === "package" ||
      (selection?.kind === "target" && !snapshot?.targets.some((t) => t.label === selection.label))
    ) {
      setSelection(null);
      return;
    }
    requestDetail();
  }

  async function loadSnapshot() {
    if (disposed) return;
    // Manual calls re-check in-flight state that acceptState already guards.
    if (snapshotRequested !== null && context.snapshotFetchedAt === snapshotRequested) return;
    snapshotRequested = context.snapshotFetchedAt;
    const generation = ++snapshotGeneration;
    update({ snapshotLoading: true, snapshotError: null });
    try {
      const snapshot = await transport.invoke<TargetIndex | null>(COMMANDS.getSnapshot);
      if (generation === snapshotGeneration) {
        acceptLoadedSnapshot(snapshot);
      }
    } catch (error) {
      failSnapshotLoad(generation, error);
    }
  }

  function failSnapshotLoad(generation: number, error: unknown) {
    if (generation !== snapshotGeneration) return;
    snapshotRequested = null;
    update({
      snapshotLoading: false,
      snapshotError: `Could not load the workspace snapshot: ${String(error)}`,
      ...(view.selection?.kind === "target"
        ? { targetDetailError: "Could not load the target list" }
        : {}),
    });
  }

  function requestDetail() {
    const label = view.selection?.kind === "target" ? view.selection.label : null;
    const snapshot = view.snapshot;
    if (
      !label ||
      disposed ||
      !snapshot ||
      view.targetDetail !== null ||
      view.targetDetailError !== null ||
      targetRequestInFlight ||
      !context.canReadDetail ||
      snapshot.meta.fetchedAt !== context.snapshotFetchedAt
    ) {
      return;
    }
    const generation = targetGeneration;
    const fetchedAt = snapshot.meta.fetchedAt;
    targetRequestInFlight = true;
    void (async () => {
      try {
        const detail = await transport.invoke<TargetDetail>(COMMANDS.getTargetDetail, {
          label,
          fetchedAt,
        });
        if (detail.target.label !== label) {
          throw new Error("Target detail does not match the selected label");
        }
        if (generation === targetGeneration) {
          update({ targetDetail: detail, targetDetailError: null });
        }
      } catch (error) {
        if (generation === targetGeneration) {
          update({ targetDetailError: String(error) });
        }
      } finally {
        targetRequestInFlight = false;
        if (generation !== targetGeneration) requestDetail();
      }
    })();
  }

  function setSelection(selection: ExplorerSelection) {
    if (disposed) return;
    if (
      selection?.kind === "package" &&
      view.selection?.kind === "package" &&
      selection.path === view.selection.path
    )
      return;
    if (
      selection?.kind === "target" &&
      view.selection?.kind === "target" &&
      selection.label === view.selection.label &&
      !view.targetDetailError
    )
      return;
    update({ selection, ...clearTargetDetail() });
    requestDetail();
  }

  function sync(next: TargetsContext): void {
    if (disposed) return;
    const contextChanged =
      next.workspace !== context.workspace || next.scopeKey !== context.scopeKey;
    const fetchedAtChanged = next.snapshotFetchedAt !== context.snapshotFetchedAt;
    context = next;
    if (contextChanged) {
      update(clearTargets());
    } else if (fetchedAtChanged) {
      update(clearTargetDetail());
    }
    const wantFetchedAt = context.snapshotFetchedAt;
    if (wantFetchedAt === null) {
      clearSnapshot();
      if (view.selection) setSelection(null);
    } else if (
      // A fetch for this snapshot may already be in flight; re-issuing would
      // duplicate the transfer this split exists to avoid.
      wantFetchedAt !== view.snapshot?.meta.fetchedAt &&
      wantFetchedAt !== snapshotRequested
    ) {
      void loadSnapshot();
    }
    requestDetail();
  }

  function dispose() {
    disposed = true;
    ++targetGeneration;
    ++snapshotGeneration;
    snapshotRequested = null;
  }

  return { sync, select: setSelection, loadSnapshot, requestDetail, dispose };
}
