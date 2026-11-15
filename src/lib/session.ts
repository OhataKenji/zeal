import { COMMANDS } from "./commands.ts";
import {
  initialModel,
  sessionView,
  update,
  type Result,
  type SessionEffect,
  type SessionMessage,
  type SessionView as SessionViewType,
  type WorkspacePage,
} from "./session_model.ts";
import type {
  AppState,
  BazelCommand,
  ExplorerSelection,
  InvocationDetail,
  InvocationRecord,
  OpenWorkspaceResult,
  Profile,
  Section,
  TargetActions,
  TargetDetail,
  TargetLoad,
  TargetOutputs,
  TargetScope,
} from "./types.ts";

export { canSend, emptyView } from "./session_model.ts";
export type { SessionView, WorkspacePage } from "./session_model.ts";

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

export function createSession(
  transport: Transport,
  changed: (view: SessionViewType) => void,
  page: WorkspacePage = "targets",
) {
  let model = initialModel(page);
  let view = sessionView(model);
  let disposed = false;
  let subscribed = false;
  let initializing: Promise<void> | undefined;
  let unlisten: (() => void) | undefined;
  let unlistenHistory: (() => void) | undefined;

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

  function dispatch(message: SessionMessage): void {
    if (disposed) return;
    const transition = update(model, message);
    model = transition.model;
    publish();
    transition.effects.forEach((effect) => void runEffect(effect).then(dispatch));
  }

  async function settle<T>(
    operation: () => Promise<T>,
    message: (result: Result<T>) => SessionMessage,
  ): Promise<SessionMessage> {
    return message(await resultOf(operation));
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

    dispatch({
      type: "stateResult",
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

  async function runEffect(effect: SessionEffect): Promise<SessionMessage> {
    switch (effect.type) {
      case "listInvocations":
        return settle(
          () =>
            transport.invoke<InvocationRecord[]>(COMMANDS.listInvocations, {
              workspace: effect.workspace,
            }),
          (result) => ({ type: "invocationsResult", requestId: effect.id, result }),
        );
      case "refresh":
        return settle(
          () =>
            transport.invoke<TargetLoad>(COMMANDS.refresh, {
              workspace: effect.workspace,
              scope: effect.scope,
            }),
          (result) => ({ type: "snapshotResult", requestId: effect.id, result }),
        );
      case "getTargetDetail": {
        return settle(
          () =>
            transport.invoke<TargetDetail>(COMMANDS.getTargetDetail, {
              workspace: effect.workspace,
              label: effect.label,
            }),
          (result) => ({ type: "targetDetailResult", requestId: effect.id, result }),
        );
      }
      case "getTargetOutputs": {
        return settle(
          () =>
            transport.invoke<TargetOutputs>(COMMANDS.getTargetOutputs, {
              workspace: effect.workspace,
              label: effect.label,
            }),
          (result) => ({ type: "targetOutputsResult", requestId: effect.id, result }),
        );
      }
      case "getTargetActions": {
        return settle(
          () =>
            transport.invoke<TargetActions>(COMMANDS.getTargetActions, {
              workspace: effect.workspace,
              label: effect.label,
            }),
          (result) => ({ type: "targetActionsResult", requestId: effect.id, result }),
        );
      }
      case "getInvocation":
        return settle(
          () =>
            transport.invoke<InvocationDetail>(COMMANDS.getInvocation, {
              workspace: effect.workspace,
              id: effect.invocationId,
            }),
          (result) => ({ type: "detailResult", requestId: effect.id, result }),
        );
      case "getProfile":
        return settle(
          () =>
            transport.invoke<Section<Profile>>(COMMANDS.getInvocationProfile, {
              workspace: effect.workspace,
              id: effect.invocationId,
            }),
          (result) => ({ type: "profileResult", requestId: effect.id, result }),
        );
      case "startInvocation":
        return {
          type: "startResult",
          requestId: effect.id,
          result: await resultOf(() =>
            transport.invoke<string>(COMMANDS.startInvocation, {
              workspace: effect.workspace,
              cmd: effect.cmd,
              pattern: effect.pattern,
              arguments: effect.argumentsText,
            }),
          ),
        };
      case "openWorkspace":
        return settle(
          () =>
            transport.invoke<OpenWorkspaceResult>(COMMANDS.openWorkspace, {
              path: effect.path,
              scope: effect.scope,
            }),
          (result) => ({ type: "workspaceResult", requestId: effect.id, result }),
        );
      case "stop":
        return settle(
          () =>
            transport.invoke<void>(COMMANDS.stopInvocation, {
              workspace: effect.workspace,
              id: effect.id,
            }),
          (result) => ({ type: "stopResult", id: effect.id, result }),
        );
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
      dispatch({ type: "start", cmd, pattern, argumentsText }),
    stop: (id: string) => dispatch({ type: "stop", id }),
    openWorkspace: (path: string, scope: TargetScope) =>
      dispatch({ type: "openWorkspace", path, scope }),
    setWorkspacePromptOpen: (open: boolean) => dispatch({ type: "workspacePrompt", open }),
    refresh: () => dispatch({ type: "refresh" }),
    selectInvocation: (id: string) => dispatch({ type: "selectInvocation", id }),
    showPage: (page: WorkspacePage) => dispatch({ type: "showPage", page }),
    loadDetail: () => dispatch({ type: "loadDetail" }),
    loadProfile: () => dispatch({ type: "loadProfile" }),
    loadInvocations: () => dispatch({ type: "loadInvocations" }),
    select: (selection: ExplorerSelection) => dispatch({ type: "select", selection }),
  };
}
