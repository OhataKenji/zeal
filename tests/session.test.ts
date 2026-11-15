import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { setImmediate } from "node:timers/promises";
import {
  canSend,
  createSession,
  emptyView,
  type Transport,
  type WorkspacePage,
} from "../src/lib/session.ts";
import type {
  AppState,
  InvocationDetail,
  InvocationRecord,
  OpenWorkspaceResult,
  OwnedInvocation,
  Profile,
  Section,
  TargetDetail,
  TargetIndex,
  WorkspaceState,
} from "../src/lib/types.ts";

const WS = "/workspace";
const OTHER = "/other";

function wsEntry(workspace: string, over: Partial<WorkspaceState> = {}): WorkspaceState {
  return {
    workspace,
    scope: { include: ["//app/..."] },
    ...over,
  };
}

function owned(
  id: string,
  workspace: string,
  over: Partial<OwnedInvocation> = {},
): OwnedInvocation {
  return {
    id,
    workspace,
    cmd: "build",
    pattern: "//:app",
    startedAt: 1,
    stoppable: true,
    stopping: false,
    console: true,
    ...over,
  };
}

function coreState(over: Partial<AppState> = {}): AppState {
  return {
    revision: 1,
    besError: null,
    besReady: true,
    bazel: "/bin/bazel",
    lastWorkspace: WS,
    workspaces: [wsEntry(WS)],
    invocations: [],
    ...over,
  };
}

const SNAPSHOT: TargetIndex = {
  meta: { fetchedAt: 1, status: "ok" },
  targets: ["//app:first", "//app:second"].map((label) => ({
    label,
    pkg: "app",
    name: label.split(":")[1],
    ruleClass: "cc_library",
    location: null,
  })),
};
const READY = coreState({
  workspaces: [wsEntry(WS)],
});
const RECORD: InvocationRecord = {
  id: "run-a",
  workspace: WS,
  cmd: "build",
  pattern: "//:app",
  startedAt: 1,
};
const COMPLETED: InvocationDetail = {
  record: { ...RECORD, finishedAt: 2, exitCode: 0 },
  log: { status: "ok", data: "same\nsame\n" },
  bep: { status: "missing" },
};
const PROFILE: Section<Profile> = { status: "ok", data: { phases: [], lanes: [] } };

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

const cleanup: Array<() => void> = [];
afterEach(() => {
  cleanup.splice(0).forEach((dispose) => dispose());
});

function harness(initial = Promise.resolve(READY), page: WorkspacePage = "targets") {
  let view = emptyView(page);
  let coreState = READY;
  let initialRequested = false;
  let stateReads = 0;
  const listeners = new Map<string, (payload: unknown) => void>();
  const pending: Array<{
    command: string;
    args?: Record<string, unknown>;
    reply: ReturnType<typeof deferred<unknown>>;
  }> = [];
  const stateRequested = deferred<void>();
  function readState(): Promise<AppState> {
    stateReads += 1;
    stateRequested.resolve();
    if (!initialRequested) {
      initialRequested = true;
      return initial;
    }
    return Promise.resolve(coreState);
  }
  const transport: Transport = {
    invoke: <T>(command: string, args?: Record<string, unknown>) => {
      if (command === "get_state") return readState() as Promise<T>;
      const reply = deferred<unknown>();
      pending.push({ command, args, reply });
      return reply.promise as Promise<T>;
    },
    listen: async <T>(name: string, callback: (payload: T) => void) => {
      listeners.set(name, callback as (payload: unknown) => void);
      return () => {
        listeners.delete(name);
      };
    },
  };
  const session = createSession(
    transport,
    (next) => {
      view = next;
    },
    page,
  );
  cleanup.push(session.dispose);
  return {
    session,
    get view() {
      return view;
    },
    listeners,
    pending,
    stateRequested: stateRequested.promise,
    stateReads: () => stateReads,
    emit: (name: string, payload: unknown) => {
      if (name === "app-state") coreState = payload as AppState;
      listeners.get(name)?.(payload);
    },
  };
}

function pendingRequest(h: ReturnType<typeof harness>, command: string) {
  const request = h.pending.find((call) => call.command === command);
  assert.ok(request, `No ${command} request is pending.`);
  return request;
}

function lastPendingRequest(h: ReturnType<typeof harness>, command: string) {
  const request = h.pending.filter((call) => call.command === command).at(-1);
  assert.ok(request, `No ${command} request is pending.`);
  return request;
}

function requestCount(h: ReturnType<typeof harness>, command: string) {
  return h.pending.filter((call) => call.command === command).length;
}

async function settle() {
  await setImmediate();
}

async function initReady(h: ReturnType<typeof harness>) {
  await h.session.init();
  await h.stateRequested;
  await settle();
}

test("initial page survives the initial state fetch", async () => {
  const h = harness(Promise.resolve(READY), "query");
  await h.session.init();
  assert.deepEqual(h.view.content, { kind: "query" });
});

test("initial restore selects lastWorkspace and loads its readers", async () => {
  const h = harness(
    Promise.resolve(
      coreState({
        workspaces: [wsEntry(WS)],
        lastWorkspace: WS,
      }),
    ),
  );
  await initReady(h);
  assert.equal(h.view.state?.workspace, WS);
  assert.equal(pendingRequest(h, "list_invocations").args?.workspace, WS);
  assert.equal(pendingRequest(h, "refresh").args?.workspace, WS);
});

test("late app-state with an older revision is ignored", async () => {
  const h = harness();
  await initReady(h);
  h.emit("app-state", coreState({ revision: 1 }));
  await settle();
  h.emit("app-state", coreState({ revision: 0, besError: "stale" }));
  await settle();
  assert.equal(h.view.core?.revision, 1);
  assert.equal(h.view.core?.besError, null);
});

test("opening B succeeds while A runs; B requests name B and A stays in core", async () => {
  const running = owned("a-run", WS);
  const h = harness(
    Promise.resolve(
      coreState({
        revision: 2,
        workspaces: [wsEntry(WS), wsEntry(OTHER)],
        invocations: [running],
      }),
    ),
  );
  await initReady(h);
  h.session.openWorkspace("/alias/other", { include: ["//..."] });
  const open = pendingRequest(h, "open_workspace");
  assert.deepEqual(open.args, {
    path: "/alias/other",
    scope: { include: ["//..."] },
  });
  const result: OpenWorkspaceResult = {
    workspace: OTHER,
    state: coreState({
      revision: 3,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
      invocations: [running],
    }),
  };
  open.reply.resolve(result);
  await settle();
  assert.equal(h.view.state?.workspace, OTHER);
  // The in-flight A history read is invalidated; its reply triggers B's read.
  pendingRequest(h, "list_invocations").reply.resolve([]);
  await settle();
  assert.equal(lastPendingRequest(h, "list_invocations").args?.workspace, OTHER);
  assert.equal(lastPendingRequest(h, "refresh").args?.workspace, OTHER);
  // A remains owned in the aggregate.
  assert.equal(h.view.core?.invocations[0]?.id, "a-run");
  // B's own start names B.
  h.session.start("build", "//app:first");
  await settle();
  assert.equal(lastPendingRequest(h, "start_invocation").args?.workspace, OTHER);
  // An app-state reporting A's completion does not navigate away from B.
  h.emit(
    "app-state",
    coreState({ revision: 4, workspaces: result.state.workspaces, invocations: [] }),
  );
  await settle();
  assert.equal(h.view.state?.workspace, OTHER);
  assert.deepEqual(h.view.content, { kind: "targets" });
});

test("a delayed A start reply cannot route to A's console or error in B", async () => {
  const h = harness();
  await initReady(h);
  h.session.start("build", "//app:first");
  await settle();
  const aStart = pendingRequest(h, "start_invocation");
  assert.equal(aStart.args?.workspace, WS);
  // User explicitly switches to B before the reply lands.
  h.session.openWorkspace(OTHER, { include: ["//..."] });
  pendingRequest(h, "open_workspace").reply.resolve({
    workspace: OTHER,
    state: coreState({
      revision: 2,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
    }),
  });
  await settle();
  assert.equal(h.view.state?.workspace, OTHER);
  aStart.reply.resolve("a-inv");
  await settle();
  assert.deepEqual(h.view.content, { kind: "targets" });
  assert.equal(h.view.error, null);
  // No fresh get_state follows the start; app-state events carry the entry.
  assert.equal(h.stateReads(), 1);
  // B can start independently while A's reply already landed.
  h.session.start("test", "//app:second");
  await settle();
  assert.equal(lastPendingRequest(h, "start_invocation").args?.workspace, OTHER);
});

test("same-workspace navigation after start is not overridden by the reply", async () => {
  const h = harness();
  await initReady(h);
  h.session.start("build", "//app:first");
  await settle();
  const start = pendingRequest(h, "start_invocation");
  h.session.showPage("query");
  start.reply.resolve("a-inv");
  await settle();
  assert.deepEqual(h.view.content, { kind: "query" });
});

test("the start reply navigates; the app-state event supplies the owned entry", async () => {
  const h = harness();
  await initReady(h);
  h.session.start(
    "run",
    "//:app",
    `--config=dev --define='message=hello world' -- "two words" '$HOME; echo'`,
  );
  await settle();
  const start = pendingRequest(h, "start_invocation");
  assert.deepEqual(start.args, {
    workspace: WS,
    cmd: "run",
    pattern: "//:app",
    arguments: `--config=dev --define='message=hello world' -- "two words" '$HOME; echo'`,
  });
  const launched = owned("a-inv", WS, { cmd: "run" });
  start.reply.resolve("a-inv");
  await settle();
  assert.deepEqual(h.view.content, { kind: "invocation", id: "a-inv", page: "targets" });
  // The reply alone carries no core entry; console and execution wait for it.
  assert.equal(h.view.console, null);
  assert.equal(h.view.execution, null);
  h.emit("app-state", coreState({ revision: 2, invocations: [launched] }));
  await settle();
  const after = h.view;
  assert.equal(after.console?.id, "a-inv");
  assert.equal(after.execution?.id, "a-inv");
  assert.equal(h.stateReads(), 1);
});

test("stop sends the owned workspace and id even after other work finishes", async () => {
  const a = owned("a-run", WS);
  const b = owned("b-run", OTHER);
  const h = harness(
    Promise.resolve(
      coreState({
        revision: 2,
        workspaces: [wsEntry(WS), wsEntry(OTHER)],
        invocations: [a, b],
      }),
    ),
  );
  await initReady(h);
  h.session.stop("a-run");
  await settle();
  const stop = pendingRequest(h, "stop_invocation");
  assert.deepEqual(stop.args, { workspace: WS, id: "a-run" });
  // B completes first — its entry leaves the map; the stop reply still
  // targets A.
  h.emit(
    "app-state",
    coreState({
      revision: 3,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
      invocations: [a],
    }),
  );
  await settle();
  stop.reply.resolve(undefined);
  await settle();
  assert.equal(h.view.error, null);
});

test("external and non-stoppable invocations expose no stop", async () => {
  const finished = owned("done", WS, { stoppable: false });
  const h = harness(Promise.resolve(coreState({ revision: 2, invocations: [finished] })));
  await initReady(h);
  h.session.stop("done");
  h.session.stop("bep-only-id");
  await settle();
  assert.equal(requestCount(h, "stop_invocation"), 0);
});

test("a live console shows until release, then the BEP record takes over", async () => {
  const live = owned("a-run", WS);
  const h = harness(Promise.resolve(coreState({ revision: 2, invocations: [live] })));
  await initReady(h);
  h.session.selectInvocation("a-run");
  await settle();
  assert.deepEqual(h.view.content, {
    kind: "invocation",
    id: "a-run",
    page: "targets",
  });
  assert.equal(h.view.console?.id, "a-run");
  assert.equal(h.view.execution?.id, "a-run");
  // The entry leaves the map; history then carries the finished record.
  h.emit("app-state", coreState({ revision: 3, invocations: [] }));
  await settle();
  assert.equal(h.view.console, null);
  assert.equal(h.view.execution, null);
  h.session.loadInvocations();
  await settle();
  pendingRequest(h, "list_invocations").reply.resolve([
    { ...RECORD, id: "a-run", finishedAt: 2, exitCode: 0 },
  ]);
  await settle();
  assert.equal(h.view.console, null);
  assert.equal(h.view.execution, null);
  assert.deepEqual(h.view.content, {
    kind: "invocation",
    id: "a-run",
    page: "targets",
  });
});

test("scope change invalidates stale target results and readers name the workspace", async () => {
  const h = harness();
  await initReady(h);
  lastPendingRequest(h, "refresh").reply.resolve({ snapshot: SNAPSHOT, error: null });
  await settle();
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  const detail = pendingRequest(h, "get_target_detail");
  assert.equal(detail.args?.workspace, WS);
  // Rescoped state arrives: selection and in-flight detail are dropped.
  h.emit(
    "app-state",
    coreState({
      revision: 2,
      workspaces: [wsEntry(WS, { scope: { include: ["//other/..."] } })],
    }),
  );
  await settle();
  assert.equal(h.view.selection, null);
  detail.reply.resolve({
    target: SNAPSHOT.targets[0],
    edges: [],
    files: [],
    status: "ok",
  } satisfies TargetDetail);
  await settle();
  assert.equal(h.view.targetDetail, null);
});

test("late history/detail/profile replies cannot populate another workspace", async () => {
  const h = harness();
  await initReady(h);
  pendingRequest(h, "list_invocations").reply.resolve([RECORD]);
  await settle();
  h.session.selectInvocation(RECORD.id);
  await settle();
  const detailReq = pendingRequest(h, "get_invocation");
  const profileReq = pendingRequest(h, "get_invocation_profile");
  assert.equal(detailReq.args?.workspace, WS);
  // Switch to B before replies land.
  h.session.openWorkspace(OTHER, { include: ["//..."] });
  pendingRequest(h, "open_workspace").reply.resolve({
    workspace: OTHER,
    state: coreState({
      revision: 2,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
    }),
  });
  await settle();
  detailReq.reply.resolve(COMPLETED);
  profileReq.reply.resolve(PROFILE);
  await settle();
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
  assert.notEqual(h.view.content.kind, "invocation");
});

test("returning to a workspace refetches and the reply loads without app-state", async () => {
  const h = harness();
  await initReady(h);
  lastPendingRequest(h, "refresh").reply.resolve({ snapshot: SNAPSHOT, error: null });
  await settle();
  assert.equal(h.view.snapshot?.meta.fetchedAt, 1);
  // Switch away and back: each selection issues its own refresh.
  h.session.openWorkspace(OTHER, { include: ["//..."] });
  pendingRequest(h, "open_workspace").reply.resolve({
    workspace: OTHER,
    state: coreState({
      revision: 2,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
    }),
  });
  await settle();
  const refreshes = requestCount(h, "refresh");
  h.session.openWorkspace(WS, { include: ["//app/..."] });
  lastPendingRequest(h, "open_workspace").reply.resolve({
    workspace: WS,
    state: coreState({
      revision: 3,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
    }),
  });
  await settle();
  assert.equal(requestCount(h, "refresh"), refreshes + 1);
  const load = lastPendingRequest(h, "refresh");
  assert.equal(load.args?.workspace, WS);
  assert.deepEqual(load.args?.scope, { include: ["//app/..."] });
  load.reply.resolve({ snapshot: SNAPSHOT, error: null });
  await settle();
  assert.equal(h.view.snapshot?.meta.fetchedAt, 1);
});

test("canSend requires readiness, a workspace, bazel, BES, and a valid target", async () => {
  const h = harness();
  await initReady(h);
  assert.equal(canSend(h.view, "//app:first"), true);
  const noBazel = { ...h.view, core: { ...READY, bazel: null } };
  assert.equal(canSend(noBazel, "//app:first"), false);
  const noBes = { ...h.view, core: { ...READY, besReady: false } };
  assert.equal(canSend(noBes, "//app:first"), false);
  const noWs = { ...h.view, state: null };
  assert.equal(canSend(noWs, "//app:first"), false);
  assert.equal(canSend(h.view, "-//bad"), false);
});

test("dispose removes listeners and ignores late reads", async () => {
  const h = harness();
  await initReady(h);
  pendingRequest(h, "list_invocations").reply.resolve([
    { ...RECORD, id: "run-a" },
    { ...RECORD, id: "run-b" },
  ]);
  await settle();
  h.session.selectInvocation("run-a");
  h.session.selectInvocation("run-b");
  await settle();
  h.session.dispose();
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.equal(h.listeners.size, 0);
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
  assert.equal(requestCount(h, "get_invocation"), 1);
  assert.equal(requestCount(h, "get_invocation_profile"), 1);
});

test("unmount during listener registration immediately cleans the late subscription", async () => {
  const registered = deferred<() => void>();
  let removed = 0;
  let invoked = false;
  const session = createSession(
    {
      listen: () => registered.promise,
      invoke: async <T>() => {
        invoked = true;
        return coreState() as T;
      },
    },
    () => {},
  );
  const initialized = session.init();
  session.dispose();
  registered.resolve(() => {
    removed++;
  });
  await initialized;
  assert.equal(removed, 1);
  assert.equal(invoked, false);
});

test("failed state subscription removes the history subscription", async () => {
  let view = emptyView();
  const removed: string[] = [];
  const session = createSession(
    {
      listen: async (name) => {
        if (name === "app-state") throw new Error("state listener unavailable");
        return () => removed.push(name);
      },
      invoke: async <T>() => coreState() as T,
    },
    (next) => {
      view = next;
    },
  );
  await session.init();
  assert.deepEqual(removed, ["invocations-changed"]);
  assert.match(view.error ?? "", /state listener unavailable/);
  session.dispose();
});
