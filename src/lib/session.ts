import { COMMANDS } from "./commands.ts";
import {
  initialModel,
  sessionView,
  update,
  type Result,
  type SessionEffect,
  type SessionMessage,
  type SessionView as SessionViewType,
} from "./session_model.ts";
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

export { bazelBusyReason, canSend, canStop, emptyView, sessionBusy } from "./session_model.ts";
export type { SessionView } from "./session_model.ts";

/** IPC boundary used by a session to call the core and subscribe to events. */
export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, callback: (payload: T) => void): Promise<() => void>;
}

async function resultOf<T>(operation: () => Promise<T>): Promise<Result<T>> {
  try {
    return { ok: true, value: await operation() };
  } catch (error) {
    return { ok: false, error: String(error) };
  }
}

function effectId(effect: SessionEffect): number {
  return "request" in effect ? effect.request.id : effect.id;
}

export function createSession(transport: Transport, changed: (view: SessionViewType) => void) {
  let model = initialModel();
  let view = sessionView(model);
  let disposed = false;
  let subscribed = false;
  let initializing: Promise<void> | undefined;
  let unlisten: (() => void) | undefined;
  let unlistenHistory: (() => void) | undefined;
  const operations = new Map<number, Promise<void>>();

  function publish() {
    const next = sessionView(model);
    if (
      Object.keys(view).every(
        (key) => view[key as keyof SessionViewType] === next[key as keyof SessionViewType],
      )
    ) {
      return;
    }
    view = next;
    changed(view);
  }

  function dispatch(message: SessionMessage): number | undefined {
    if (disposed) return;
    const transition = update(model, message);
    model = transition.model;
    publish();
    transition.effects.forEach(launch);
    return transition.waitFor;
  }

  async function settle<T>(
    operation: () => Promise<T>,
    message: (result: Result<T>) => SessionMessage,
  ) {
    dispatch(message(await resultOf(operation)));
  }

  function waitFor(id: number | undefined): Promise<void> {
    return id === undefined ? Promise.resolve() : (operations.get(id) ?? Promise.resolve());
  }

  function launch(effect: SessionEffect) {
    const id = effectId(effect);
    if (operations.has(id)) return;
    let operation: Promise<void>;
    operation = runEffect(effect).finally(() => {
      if (operations.get(id) === operation) operations.delete(id);
    });
    operations.set(id, operation);
  }

  async function initialize() {
    try {
      const removeHistory = await transport.listen<void>("invocations-changed", () => {
        dispatch({ type: "historyChanged" });
      });
      if (disposed) {
        removeHistory();
        return;
      }
      unlistenHistory = removeHistory;
      const removeState = await transport.listen<AppState>("app-state", (state) => {
        dispatch({ type: "appState", state });
      });
      if (disposed) {
        removeState();
        return;
      }
      unlisten = removeState;
      subscribed = true;
    } catch (error) {
      unlistenHistory?.();
      unlistenHistory = undefined;
      if (!disposed) dispatch({ type: "subscriptionsFailed", error: String(error) });
      return;
    }

    const revision = model.stateRevision;
    dispatch({
      type: "initialStateResult",
      revision,
      result: await resultOf(() => transport.invoke<AppState>(COMMANDS.getState)),
    });
  }

  function init(): Promise<void> {
    if (initializing) return initializing;
    if (subscribed || disposed) return Promise.resolve();
    initializing = initialize().finally(() => {
      initializing = undefined;
    });
    return initializing;
  }

  async function runEffect(effect: SessionEffect): Promise<void> {
    switch (effect.type) {
      case "listInvocations":
        await settle(
          () => transport.invoke<InvocationRecord[]>(COMMANDS.listInvocations),
          (result) => ({ type: "invocationsResult", requestId: effect.request.id, result }),
        );
        return;
      case "getSnapshot":
        await settle(
          () => transport.invoke<TargetIndex | null>(COMMANDS.getSnapshot),
          (result) => ({ type: "snapshotResult", requestId: effect.request.id, result }),
        );
        return;
      case "getTargetDetail": {
        const { label, fetchedAt } = effect.request;
        await settle(
          () => transport.invoke<TargetDetail>(COMMANDS.getTargetDetail, { label, fetchedAt }),
          (result) => ({ type: "targetDetailResult", requestId: effect.request.id, result }),
        );
        return;
      }
      case "getTargetOutputs": {
        const { label, fetchedAt } = effect.request;
        await settle(
          () => transport.invoke<TargetOutputs>(COMMANDS.getTargetOutputs, { label, fetchedAt }),
          (result) => ({ type: "targetOutputsResult", requestId: effect.request.id, result }),
        );
        return;
      }
      case "getInvocation":
        await settle(
          () =>
            transport.invoke<InvocationDetail>(COMMANDS.getInvocation, {
              id: effect.request.invocationId,
            }),
          (result) => ({ type: "detailResult", requestId: effect.request.id, result }),
        );
        return;
      case "getProfile":
        await settle(
          () =>
            transport.invoke<Section<Profile>>(COMMANDS.getInvocationProfile, {
              id: effect.request.invocationId,
            }),
          (result) => ({ type: "profileResult", requestId: effect.request.id, result }),
        );
        return;
      case "startInvocation": {
        const { request } = effect;
        const wait = dispatch({
          type: "startResult",
          requestId: request.id,
          result: await resultOf(() =>
            transport.invoke<InvocationRecord>(COMMANDS.startInvocation, {
              cmd: request.cmd,
              pattern: request.pattern,
              arguments: request.argumentsText,
            }),
          ),
        });
        await waitFor(wait);
        return;
      }
      case "getStartState":
        await settle(
          () => transport.invoke<AppState>(COMMANDS.getState),
          (result) => ({
            type: "startStateResult",
            startId: effect.startId,
            revision: effect.revision,
            result,
          }),
        );
        return;
      case "openWorkspace": {
        const { request } = effect;
        try {
          const state = await transport.invoke<AppState>(COMMANDS.openWorkspace, {
            path: request.path,
            scope: request.scope,
          });
          dispatch({
            type: "workspaceResult",
            requestId: request.id,
            result: { ok: true, value: state },
          });
        } catch (error) {
          dispatch({
            type: "workspaceResult",
            requestId: request.id,
            result: { ok: false, error: String(error) },
          });
          throw error;
        }
        return;
      }
      case "rejectWorkspaceOpen":
        throw new Error(effect.message);
      case "stop":
        await settle(
          () => transport.invoke<void>(COMMANDS.stopInvocation),
          (result) => ({ type: "stopResult", workspace: effect.workspace, result }),
        );
        return;
      case "refresh":
        await settle(
          () => transport.invoke<void>(COMMANDS.refresh),
          (result) => ({ type: "refreshResult", workspace: effect.workspace, result }),
        );
        return;
    }
  }

  function dispose() {
    disposed = true;
    unlisten?.();
    unlisten = undefined;
    unlistenHistory?.();
    unlistenHistory = undefined;
  }

  return {
    init,
    dispose,
    start: (cmd: BazelCommand, pattern: string, argumentsText = "") =>
      waitFor(dispatch({ type: "start", cmd, pattern, argumentsText })),
    stop: () => waitFor(dispatch({ type: "stop" })),
    openWorkspace: (path: string, scope: TargetScope) =>
      waitFor(dispatch({ type: "openWorkspace", path, scope })),
    refresh: () => waitFor(dispatch({ type: "refresh" })),
    selectInvocation: (id: string | null) => dispatch({ type: "selectInvocation", id }),
    openConsole: (record: InvocationRecord) => dispatch({ type: "openConsole", record }),
    loadDetail: () => waitFor(dispatch({ type: "loadDetail" })),
    loadProfile: () => waitFor(dispatch({ type: "loadProfile" })),
    loadInvocations: () => waitFor(dispatch({ type: "loadInvocations" })),
    loadSnapshot: () => waitFor(dispatch({ type: "loadSnapshot" })),
    select: (selection: ExplorerSelection) => dispatch({ type: "select", selection }),
  };
}
