import { COMMANDS } from "./commands.ts";
import {
  initialModel,
  sessionView,
  update,
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

function unexpected(effect: never): never {
  throw new Error(`Unhandled session effect: ${JSON.stringify(effect)}`);
}

export function createSession(transport: Transport, changed: (view: SessionViewType) => void) {
  let model = initialModel();
  let view = sessionView(model);
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let unlistenHistory: (() => void) | undefined;
  const operations = new Map<number, Promise<void>>();

  function publish() {
    const next = sessionView(model);
    const keys = Object.keys(view) as Array<keyof SessionViewType>;
    if (keys.every((key) => view[key] === next[key])) return;
    view = next;
    changed(view);
  }

  function dispatch(message: SessionMessage): number | undefined {
    if (disposed) return undefined;
    const transition = update(model, message);
    model = transition.model;
    publish();
    transition.effects.forEach(launch);
    return transition.waitFor;
  }

  function waitFor(requestId: number | undefined): Promise<void> {
    return requestId === undefined
      ? Promise.resolve()
      : (operations.get(requestId) ?? Promise.resolve());
  }

  function launch(effect: SessionEffect) {
    if (operations.has(effect.requestId)) return;
    let operation: Promise<void>;
    operation = runEffect(effect).finally(() => {
      if (operations.get(effect.requestId) === operation) {
        operations.delete(effect.requestId);
      }
    });
    operations.set(effect.requestId, operation);
  }

  async function subscribe(requestId: number) {
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
      await waitFor(dispatch({ type: "subscriptionsReady", requestId }));
    } catch (error) {
      unlistenHistory?.();
      unlistenHistory = undefined;
      if (!disposed) {
        dispatch({
          type: "subscriptionsFailed",
          requestId,
          error: String(error),
        });
      }
    }
  }

  async function runEffect(effect: SessionEffect): Promise<void> {
    switch (effect.type) {
      case "subscribe": {
        await subscribe(effect.requestId);
        return;
      }
      case "getInitialState": {
        try {
          const state = await transport.invoke<AppState>(COMMANDS.getState);
          dispatch({
            type: "initialStateLoaded",
            requestId: effect.requestId,
            revision: effect.revision,
            state,
          });
        } catch (error) {
          dispatch({
            type: "initialStateFailed",
            requestId: effect.requestId,
            revision: effect.revision,
            error: String(error),
          });
        }
        return;
      }
      case "listInvocations": {
        try {
          const records = await transport.invoke<InvocationRecord[]>(COMMANDS.listInvocations);
          dispatch({ type: "invocationsLoaded", requestId: effect.requestId, records });
        } catch (error) {
          dispatch({
            type: "invocationsFailed",
            requestId: effect.requestId,
            error: String(error),
          });
        }
        return;
      }
      case "getSnapshot": {
        try {
          const snapshot = await transport.invoke<TargetIndex | null>(COMMANDS.getSnapshot);
          dispatch({ type: "snapshotLoaded", requestId: effect.requestId, snapshot });
        } catch (error) {
          dispatch({
            type: "snapshotFailed",
            requestId: effect.requestId,
            error: String(error),
          });
        }
        return;
      }
      case "getTargetDetail": {
        const { label, fetchedAt } = effect.request;
        try {
          const detail = await transport.invoke<TargetDetail>(COMMANDS.getTargetDetail, {
            label,
            fetchedAt,
          });
          dispatch({ type: "targetDetailLoaded", requestId: effect.requestId, detail });
        } catch (error) {
          dispatch({
            type: "targetDetailFailed",
            requestId: effect.requestId,
            error: String(error),
          });
        }
        return;
      }
      case "getTargetOutputs": {
        const { label, fetchedAt } = effect.request;
        try {
          const outputs = await transport.invoke<TargetOutputs>(COMMANDS.getTargetOutputs, {
            label,
            fetchedAt,
          });
          dispatch({ type: "targetOutputsLoaded", requestId: effect.requestId, outputs });
        } catch (error) {
          dispatch({
            type: "targetOutputsFailed",
            requestId: effect.requestId,
            error: String(error),
          });
        }
        return;
      }
      case "getInvocation": {
        try {
          const detail = await transport.invoke<InvocationDetail>(COMMANDS.getInvocation, {
            id: effect.request.invocationId,
          });
          dispatch({ type: "detailLoaded", requestId: effect.requestId, detail });
        } catch (error) {
          dispatch({
            type: "detailFailed",
            requestId: effect.requestId,
            error: String(error),
          });
        }
        return;
      }
      case "getProfile": {
        try {
          const profile = await transport.invoke<Section<Profile>>(COMMANDS.getInvocationProfile, {
            id: effect.request.invocationId,
          });
          dispatch({ type: "profileLoaded", requestId: effect.requestId, profile });
        } catch (error) {
          dispatch({
            type: "profileFailed",
            requestId: effect.requestId,
            error: String(error),
          });
        }
        return;
      }
      case "startInvocation": {
        const request = effect.request;
        try {
          const record = await transport.invoke<InvocationRecord>(COMMANDS.startInvocation, {
            cmd: request.cmd,
            pattern: request.pattern,
            arguments: request.argumentsText,
          });
          await waitFor(
            dispatch({
              type: "startReturned",
              requestId: request.id,
              token: request.token,
              record,
            }),
          );
        } catch (error) {
          dispatch({
            type: "startFailed",
            requestId: request.id,
            token: request.token,
            error: String(error),
          });
        }
        return;
      }
      case "getStartState": {
        try {
          const state = await transport.invoke<AppState>(COMMANDS.getState);
          dispatch({
            type: "startStateLoaded",
            requestId: effect.requestId,
            startRequestId: effect.startRequestId,
            token: effect.token,
            revision: effect.revision,
            state,
          });
        } catch (error) {
          dispatch({
            type: "startStateFailed",
            requestId: effect.requestId,
            startRequestId: effect.startRequestId,
            token: effect.token,
            revision: effect.revision,
            error: String(error),
          });
        }
        return;
      }
      case "openWorkspace": {
        const request = effect.request;
        try {
          const state = await transport.invoke<AppState>(COMMANDS.openWorkspace, {
            path: request.path,
            scope: request.scope,
          });
          dispatch({
            type: "workspaceOpened",
            requestId: request.id,
            revision: request.revision,
            state,
          });
        } catch (error) {
          dispatch({ type: "workspaceOpenFailed", requestId: request.id });
          throw error;
        }
        return;
      }
      case "rejectWorkspaceOpen": {
        throw new Error(effect.message);
      }
      case "stop": {
        try {
          await transport.invoke(COMMANDS.stopInvocation);
        } catch (error) {
          dispatch({
            type: "stopFailed",
            workspace: effect.workspace,
            error: String(error),
          });
        }
        return;
      }
      case "refresh": {
        try {
          await transport.invoke(COMMANDS.refresh);
        } catch (error) {
          dispatch({
            type: "refreshFailed",
            workspace: effect.workspace,
            error: String(error),
          });
        }
        return;
      }
      default: {
        return unexpected(effect);
      }
    }
  }

  function init(): Promise<void> {
    const requestId = dispatch({ type: "init" }) ?? model.subscriptionRequestId ?? undefined;
    return waitFor(requestId);
  }

  function dispose() {
    disposed = true;
    unlisten?.();
    unlisten = undefined;
    unlistenHistory?.();
    unlistenHistory = undefined;
  }

  function loadInvocations(): Promise<void> {
    return waitFor(dispatch({ type: "loadInvocations" }));
  }

  function loadSnapshot(): Promise<void> {
    return waitFor(dispatch({ type: "loadSnapshot" }));
  }

  function loadDetail(): Promise<void> {
    return waitFor(dispatch({ type: "loadDetail" }));
  }

  function loadProfile(): Promise<void> {
    return waitFor(dispatch({ type: "loadProfile" }));
  }

  function start(cmd: BazelCommand, pattern: string, argumentsText = ""): Promise<void> {
    const requestId = dispatch({ type: "start", cmd, pattern, argumentsText });
    return waitFor(requestId);
  }

  function stop(): Promise<void> {
    return waitFor(dispatch({ type: "stop" }));
  }

  function openWorkspace(path: string, scope: TargetScope): Promise<void> {
    return waitFor(dispatch({ type: "openWorkspace", path, scope }));
  }

  function refresh(): Promise<void> {
    return waitFor(dispatch({ type: "refresh" }));
  }

  function select(selection: ExplorerSelection) {
    dispatch({ type: "select", selection });
  }

  function selectInvocation(id: string | null) {
    dispatch({ type: "selectInvocation", id });
  }

  function openConsole(record: InvocationRecord) {
    dispatch({ type: "openConsole", record });
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
