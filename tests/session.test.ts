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
const OTHER_RECORD: InvocationRecord = { ...RECORD, id: "b-run", workspace: OTHER };
const COMPLETED_RECORD: InvocationRecord = { ...RECORD, finishedAt: 2, exitCode: 0 };
const THIRD = "/third";

function targetDetail(label: string): TargetDetail {
  const target = SNAPSHOT.targets.find((item) => item.label === label);
  assert.ok(target);
  return { target, edges: [], files: [], status: "ok" };
}

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

async function loadSnapshot(h: ReturnType<typeof harness>) {
  lastPendingRequest(h, "refresh").reply.resolve({ snapshot: SNAPSHOT, error: null });
  await settle();
}

async function loadHistory(h: ReturnType<typeof harness>, records: InvocationRecord[]) {
  lastPendingRequest(h, "list_invocations").reply.resolve(records);
  await settle();
}

test("initial page survives the initial state fetch", async () => {
  const h = harness(Promise.resolve(READY), "query");
  await h.session.init();
  assert.deepEqual(h.view.content, { kind: "query" });
});

test("initial restore selects lastWorkspace and loads its readers", async () => {
  const h = harness(
    Promise.resolve(coreState({ revision: 1, workspaces: [], lastWorkspace: null })),
  );
  await initReady(h);
  const noSelection = h.view.state;
  assert.equal(noSelection, null);
  h.emit("app-state", coreState({ revision: 2, workspaces: [wsEntry(WS)], lastWorkspace: WS }));
  await settle();
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
  void h.session.openWorkspace("/alias/other", { include: ["//..."] });
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
  // B's reads start while A's are still in flight; A's late replies cannot
  // populate B.
  assert.ok(
    h.pending.some((call) => call.command === "list_invocations" && call.args?.workspace === OTHER),
  );
  pendingRequest(h, "list_invocations").reply.resolve([{ ...RECORD, id: "a-done" }]);
  await settle();
  assert.deepEqual(h.view.invocations, []);
  lastPendingRequest(h, "list_invocations").reply.resolve([OTHER_RECORD]);
  await settle();
  assert.deepEqual(h.view.invocations, [OTHER_RECORD]);
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
  void h.session.openWorkspace(OTHER, { include: ["//..."] });
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
  // A rejected reply after navigation likewise shows no stale error.
  h.session.start("build", "//app:second");
  await settle();
  const second = lastPendingRequest(h, "start_invocation");
  h.session.showPage("analytics");
  second.reply.reject(new Error("spawn failed"));
  await settle();
  assert.equal(h.view.error, null);
  assert.deepEqual(h.view.content, { kind: "analytics" });
});

test("a start reply after browsing away and back does not navigate", async () => {
  const h = harness();
  await initReady(h);
  h.session.start("build", "//app:first");
  await settle();
  const start = pendingRequest(h, "start_invocation");
  h.session.showPage("query");
  h.session.showPage("targets");
  start.reply.resolve("a-inv");
  await settle();
  assert.deepEqual(h.view.content, { kind: "targets" });
  assert.equal(h.view.error, null);
});

test("a start reply after a rejected workspace open does not navigate or error", async () => {
  const h = harness();
  await initReady(h);
  h.session.start("build", "//app:first");
  await settle();
  const start = pendingRequest(h, "start_invocation");
  const opened = h.session.openWorkspace(OTHER, { include: ["//..."] });
  const rejection = assert.rejects(opened, /open failed/);
  pendingRequest(h, "open_workspace").reply.reject(new Error("open failed"));
  await rejection;
  assert.equal(h.view.state?.workspace, WS);
  start.reply.resolve("a-inv");
  await settle();
  assert.deepEqual(h.view.content, { kind: "targets" });
  assert.equal(h.view.error, null);
});

test("a start reply after reselecting targets does not navigate", async () => {
  const h = harness();
  await initReady(h);
  await loadSnapshot(h);
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  h.session.start("build", "//app:first");
  await settle();
  const start = pendingRequest(h, "start_invocation");
  h.session.select({ kind: "target", label: "//app:second" });
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  start.reply.resolve("a-inv");
  await settle();
  assert.deepEqual(h.view.content, { kind: "targets" });
  assert.deepEqual(h.view.selection, { kind: "target", label: "//app:first" });
  assert.equal(h.view.starting, false);
  assert.equal(h.view.error, null);
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
  // The reply alone carries no core entry.
  assert.equal(h.view.invocation, null);
  h.emit("app-state", coreState({ revision: 2, invocations: [launched] }));
  await settle();
  const after = h.view;
  assert.deepEqual(after.invocation, { record: launched, execution: launched });
  // The state push derives the console in place; it does not navigate.
  assert.deepEqual(after.content, { kind: "invocation", id: "a-inv", page: "targets" });
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

test("a live run in shared history opens the owner's console and stops at its owner", async () => {
  const other = owned("b-run", OTHER, { cmd: "run" });
  const h = harness(
    Promise.resolve(
      coreState({
        workspaces: [wsEntry(WS), wsEntry(OTHER)],
        invocations: [other],
      }),
    ),
  );
  await initReady(h);
  const record = { ...OTHER_RECORD, cmd: "run", exitCode: 0, finishedAt: 2 };
  await loadHistory(h, [record]);
  assert.deepEqual(h.view.invocationRows, [{ record, execution: other }]);
  h.session.selectInvocation(record.id);
  assert.deepEqual(h.view.invocation, { record, execution: other });
  assert.equal(requestCount(h, "get_invocation"), 0);
  const stopped = h.session.stop(record.id);
  pendingRequest(h, "stop_invocation").reply.resolve(undefined);
  await stopped;
  assert.deepEqual(pendingRequest(h, "stop_invocation").args, { workspace: OTHER, id: record.id });
});

test("history refresh preserves a newly selected shared run through release", async () => {
  const execution = owned("b-run", OTHER, { cmd: "run" });
  const state = coreState({ workspaces: [wsEntry(WS), wsEntry(OTHER)], invocations: [execution] });
  const h = harness(Promise.resolve(state));
  await initReady(h);
  const record = { ...OTHER_RECORD, cmd: "run", exitCode: 0 };
  await loadHistory(h, [record]);

  h.session.loadInvocations();
  h.session.selectInvocation(record.id);
  // The in-flight request predates selection and lacks the selected record.
  await loadHistory(h, [COMPLETED_RECORD]);
  assert.deepEqual(h.view.invocation, { record, execution });
  assert.deepEqual(lastPendingRequest(h, "list_invocations").args, {
    workspace: WS,
    selectedId: record.id,
  });

  h.emit("app-state", { ...state, revision: state.revision + 1, invocations: [] });
  await loadHistory(h, [COMPLETED_RECORD, record]);
  assert.deepEqual(h.view.invocation?.record, record);
  assert.equal(h.view.invocation?.execution, undefined);
  assert.deepEqual(pendingRequest(h, "get_invocation").args, { workspace: WS, id: record.id });
  assert.deepEqual(pendingRequest(h, "get_invocation_profile").args, {
    workspace: WS,
    id: record.id,
  });
});

test("history must match both the ID and workspace to expose a foreign live execution", async () => {
  const other = owned("b-run", OTHER);
  const hidden = owned("invisible", OTHER);
  const h = harness(
    Promise.resolve(
      coreState({
        workspaces: [wsEntry(WS), wsEntry(OTHER)],
        invocations: [other, hidden],
      }),
    ),
  );
  await initReady(h);
  const record = { ...OTHER_RECORD, workspace: WS };
  await loadHistory(h, [record]);
  assert.equal(h.view.invocationRows.length, 1);
  assert.equal(h.view.invocationRows[0]?.execution, undefined);
  h.session.selectInvocation(record.id);
  assert.equal(h.view.invocation?.execution, undefined);
  await h.session.stop(record.id);
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
  const running = h.view;
  assert.deepEqual(running.invocation, { record: live, execution: live });
  // The entry leaves the map; history then carries the finished record.
  h.emit("app-state", coreState({ revision: 3, invocations: [] }));
  await settle();
  const released = h.view;
  assert.equal(released.invocation, null);
  h.session.loadInvocations();
  await settle();
  pendingRequest(h, "list_invocations").reply.resolve([
    { ...RECORD, id: "a-run", finishedAt: 2, exitCode: 0 },
  ]);
  await settle();
  assert.equal(h.view.invocation?.record.exitCode, 0);
  assert.equal(h.view.invocation?.execution, undefined);
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
  // A manual refresh under the old scope is still in flight at the change.
  h.session.refresh();
  await settle();
  const oldScopeRefresh = lastPendingRequest(h, "refresh");
  assert.deepEqual(oldScopeRefresh.args?.scope, { include: ["//app/..."] });
  // Rescoped state arrives: selection, snapshot, and in-flight detail drop.
  h.emit(
    "app-state",
    coreState({
      revision: 2,
      workspaces: [wsEntry(WS, { scope: { include: ["//other/..."] } })],
    }),
  );
  await settle();
  const newScopeRefresh = lastPendingRequest(h, "refresh");
  assert.deepEqual(newScopeRefresh.args?.scope, { include: ["//other/..."] });
  assert.equal(h.view.selection, null);
  const clearedSnapshot = h.view.snapshot;
  assert.equal(clearedSnapshot, null);
  newScopeRefresh.reply.resolve({
    snapshot: { ...SNAPSHOT, meta: { fetchedAt: 2, status: "ok" }, targets: [] },
    error: null,
  });
  await settle();
  // Late replies cannot publish: neither the old-scope refresh nor the old
  // detail may replace the new snapshot or restart the old target chain.
  oldScopeRefresh.reply.resolve({ snapshot: SNAPSHOT, error: null });
  detail.reply.resolve({
    target: SNAPSHOT.targets[0],
    edges: [],
    files: [],
    status: "ok",
  } satisfies TargetDetail);
  await settle();
  assert.equal(h.view.snapshot?.meta.fetchedAt, 2);
  assert.equal(h.view.snapshot?.targets.length, 0);
  assert.equal(h.view.selection, null);
  assert.equal(h.view.targetDetail, null);
  assert.equal(requestCount(h, "get_target_outputs"), 0);
  assert.equal(requestCount(h, "get_target_actions"), 0);
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
  void h.session.openWorkspace(OTHER, { include: ["//..."] });
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
  void h.session.openWorkspace(OTHER, { include: ["//..."] });
  pendingRequest(h, "open_workspace").reply.resolve({
    workspace: OTHER,
    state: coreState({
      revision: 2,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
    }),
  });
  await settle();
  const refreshes = requestCount(h, "refresh");
  void h.session.openWorkspace(WS, { include: ["//app/..."] });
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
  await settle();
  const detailA = pendingRequest(h, "get_invocation");
  const profileA = pendingRequest(h, "get_invocation_profile");
  h.session.selectInvocation("run-b");
  await settle();
  const detailB = lastPendingRequest(h, "get_invocation");
  const profileB = lastPendingRequest(h, "get_invocation_profile");
  // Each selection issued its own saved reads; A's were orphaned by B's.
  assert.equal(requestCount(h, "get_invocation"), 2);
  assert.equal(requestCount(h, "get_invocation_profile"), 2);
  h.session.dispose();
  // Late replies cannot publish values or start further work.
  detailA.reply.resolve(COMPLETED);
  profileA.reply.resolve(PROFILE);
  detailB.reply.resolve({
    ...COMPLETED,
    record: { ...COMPLETED.record, id: "run-b" },
  });
  profileB.reply.resolve(PROFILE);
  await settle();
  assert.equal(h.listeners.size, 0);
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
  assert.equal(requestCount(h, "get_invocation"), 2);
  assert.equal(requestCount(h, "get_invocation_profile"), 2);
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

test("outstanding refresh replies cannot overwrite the latest workspace's snapshot", async () => {
  const h = harness(
    Promise.resolve(coreState({ revision: 2, workspaces: [wsEntry(WS), wsEntry(OTHER)] })),
  );
  await initReady(h);
  await loadSnapshot(h);
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  // A manual refresh stays in flight across both workspace switches.
  h.session.refresh();
  await settle();
  const abandonedA = lastPendingRequest(h, "refresh");
  assert.equal(abandonedA.args?.workspace, WS);
  void h.session.openWorkspace(OTHER, { include: ["//..."] });
  pendingRequest(h, "open_workspace").reply.resolve({
    workspace: OTHER,
    state: coreState({
      revision: 3,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
    }),
  });
  await settle();
  const staleB = lastPendingRequest(h, "refresh");
  assert.equal(staleB.args?.workspace, OTHER);
  void h.session.openWorkspace(WS, { include: ["//app/..."] });
  lastPendingRequest(h, "open_workspace").reply.resolve({
    workspace: WS,
    state: coreState({
      revision: 4,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
    }),
  });
  await settle();
  const currentA = lastPendingRequest(h, "refresh");
  assert.equal(currentA.args?.workspace, WS);
  assert.deepEqual(currentA.args?.scope, { include: ["//app/..."] });
  // Older replies and errors are dropped; only the newest A reply lands.
  staleB.reply.resolve({ snapshot: null, error: { exitCode: 1, summary: "stale B" } });
  abandonedA.reply.reject(new Error("old A failed"));
  await settle();
  const droppedSnapshot = h.view.snapshot;
  assert.equal(droppedSnapshot, null);
  assert.equal(h.view.snapshotError, null);
  currentA.reply.resolve({ snapshot: SNAPSHOT, error: null });
  await settle();
  assert.equal(h.view.snapshot?.meta.fetchedAt, 1);
  assert.equal(h.view.snapshotError, null);
});

test("an accepted refresh re-reads the selected target even when fetchedAt is unchanged", async () => {
  const h = harness();
  await initReady(h);
  await loadSnapshot(h);
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  const staleDetail = pendingRequest(h, "get_target_detail");
  h.session.refresh();
  await settle();
  lastPendingRequest(h, "refresh").reply.resolve({
    snapshot: { ...SNAPSHOT, meta: { fetchedAt: 1, status: "ok" } },
    error: null,
  });
  await settle();
  staleDetail.reply.resolve(targetDetail("//app:first"));
  await settle();
  const droppedDetail = h.view.targetDetail;
  assert.equal(droppedDetail, null);
  const freshDetail = lastPendingRequest(h, "get_target_detail");
  assert.equal(freshDetail.args?.label, "//app:first");
  freshDetail.reply.resolve(targetDetail("//app:first"));
  await settle();
  assert.equal(h.view.targetDetail?.target.label, "//app:first");
});

test("a failed foreground start shows the error and restores send readiness", async () => {
  const h = harness();
  await initReady(h);
  h.session.start("build", "//app:first");
  await settle();
  pendingRequest(h, "start_invocation").reply.reject(new Error("spawn failed"));
  await settle();
  assert.match(h.view.error ?? "", /spawn failed/);
  assert.equal(h.view.starting, false);
  assert.deepEqual(h.view.content, { kind: "targets" });
  assert.equal(canSend(h.view, "//app:first"), true);
});

test("a failed A start cannot clear B's pending start", async () => {
  const h = harness(
    Promise.resolve(coreState({ revision: 2, workspaces: [wsEntry(WS), wsEntry(OTHER)] })),
  );
  await initReady(h);
  h.session.start("build", "//app:first");
  await settle();
  const aStart = pendingRequest(h, "start_invocation");
  assert.equal(aStart.args?.workspace, WS);
  void h.session.openWorkspace(OTHER, { include: ["//..."] });
  pendingRequest(h, "open_workspace").reply.resolve({
    workspace: OTHER,
    state: coreState({
      revision: 3,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
    }),
  });
  await settle();
  h.session.start("build", "//app:first");
  await settle();
  const bStart = lastPendingRequest(h, "start_invocation");
  assert.equal(bStart.args?.workspace, OTHER);
  aStart.reply.reject(new Error("spawn failed"));
  await settle();
  assert.equal(h.view.starting, true);
  assert.equal(h.view.error, null);
  bStart.reply.resolve("inv-b");
  await settle();
  assert.deepEqual(h.view.content, { kind: "invocation", id: "inv-b", page: "targets" });
});

test("a start reply after release renders the saved result and reads for the same id", async () => {
  const h = harness();
  await initReady(h);
  await loadHistory(h, []);
  h.session.start("build", "//app:first");
  await settle();
  const start = pendingRequest(h, "start_invocation");
  // The worker already finished: its entry arrived and left, and history
  // carries the completed record before the start reply lands.
  h.emit("app-state", coreState({ revision: 2, invocations: [owned("inv-x", WS)] }));
  await settle();
  h.emit("app-state", coreState({ revision: 3, invocations: [] }));
  await settle();
  h.emit("invocations-changed", null);
  await settle();
  await loadHistory(h, [{ ...RECORD, id: "inv-x", finishedAt: 2, exitCode: 0 }]);
  start.reply.resolve("inv-x");
  await settle();
  assert.deepEqual(h.view.content, { kind: "invocation", id: "inv-x", page: "targets" });
  assert.equal(h.view.invocation?.record.exitCode, 0);
  assert.equal(h.view.invocation?.execution, undefined);
  const detail = pendingRequest(h, "get_invocation");
  const profile = pendingRequest(h, "get_invocation_profile");
  assert.equal(detail.args?.workspace, WS);
  assert.equal(detail.args?.id, "inv-x");
  assert.equal(profile.args?.workspace, WS);
  assert.equal(profile.args?.id, "inv-x");
});

test("a live Console suppresses saved reads until the entry is released", async () => {
  const live = owned("a-run", WS);
  const h = harness(Promise.resolve(coreState({ revision: 2, invocations: [live] })));
  await initReady(h);
  await loadHistory(h, [
    { ...RECORD, id: "a-run", finishedAt: 2, exitCode: 0 },
    { ...RECORD, id: "old-1" },
  ]);
  // Live entries pin first and prefer their saved record for display.
  assert.equal(h.view.invocationRows.length, 2);
  assert.equal(h.view.invocationRows[0]?.execution?.id, "a-run");
  assert.equal(h.view.invocationRows[0]?.record.finishedAt, 2);
  assert.equal(h.view.invocationRows[1]?.record.id, "old-1");
  assert.equal(h.view.invocationRows[1]?.execution, undefined);
  h.session.selectInvocation("a-run");
  await settle();
  assert.equal(h.view.invocation?.execution?.console, true);
  // The BEP record exists, but the live Console keeps saved reads ineligible.
  assert.equal(requestCount(h, "get_invocation"), 0);
  assert.equal(requestCount(h, "get_invocation_profile"), 0);
  h.emit("app-state", coreState({ revision: 3, invocations: [] }));
  await settle();
  assert.equal(h.view.invocation?.record.exitCode, 0);
  assert.equal(h.view.invocation?.execution, undefined);
  assert.deepEqual(h.view.content, { kind: "invocation", id: "a-run", page: "targets" });
  const detail = pendingRequest(h, "get_invocation");
  const profile = pendingRequest(h, "get_invocation_profile");
  assert.equal(detail.args?.workspace, WS);
  assert.equal(detail.args?.id, "a-run");
  assert.equal(profile.args?.workspace, WS);
  assert.equal(profile.args?.id, "a-run");
});

test("saved reads require the record in history, not just an owned entry", async () => {
  const h = harness(
    Promise.resolve(
      coreState({
        revision: 2,
        invocations: [owned("q-1", WS, { cmd: "query", console: false })],
      }),
    ),
  );
  await initReady(h);
  await loadHistory(h, []);
  h.session.selectInvocation("q-1");
  await settle();
  assert.deepEqual(h.view.content, { kind: "invocation", id: "q-1", page: "targets" });
  assert.equal(h.view.invocation?.execution?.id, "q-1");
  assert.equal(requestCount(h, "get_invocation"), 0);
  assert.equal(requestCount(h, "get_invocation_profile"), 0);
});

test("releasing an invocation elsewhere never moves the current destination", async () => {
  const live = owned("a-run", WS);
  const h = harness(Promise.resolve(coreState({ revision: 2, invocations: [live] })));
  await initReady(h);
  h.session.selectInvocation("a-run");
  await settle();
  h.session.showPage("query");
  h.emit("app-state", coreState({ revision: 3, invocations: [] }));
  await settle();
  assert.deepEqual(h.view.content, { kind: "query" });
  assert.equal(h.view.invocation, null);
});

test("rapid target selection queues the newest target without concurrent detail reads", async () => {
  const h = harness();
  await initReady(h);
  await loadSnapshot(h);
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  const first = pendingRequest(h, "get_target_detail");
  assert.equal(first.args?.label, "//app:first");
  h.session.select({ kind: "target", label: "//app:second" });
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  // The newest selection waits; no second detail call runs concurrently.
  assert.equal(requestCount(h, "get_target_detail"), 1);
  first.reply.resolve(targetDetail("//app:first"));
  await settle();
  // The stale detail never launches outputs/actions for the old context.
  assert.equal(requestCount(h, "get_target_outputs"), 0);
  assert.equal(requestCount(h, "get_target_actions"), 0);
  const latest = lastPendingRequest(h, "get_target_detail");
  assert.equal(latest.args?.label, "//app:first");
  latest.reply.resolve(targetDetail("//app:first"));
  await settle();
  const outputs = pendingRequest(h, "get_target_outputs");
  assert.equal(outputs.args?.label, "//app:first");
  outputs.reply.resolve({ paths: ["out"], truncated: false });
  await settle();
  const actions = pendingRequest(h, "get_target_actions");
  assert.equal(actions.args?.label, "//app:first");
  actions.reply.resolve({ actions: [], truncated: false });
  await settle();
  assert.equal(h.view.targetDetail?.target.label, "//app:first");
  assert.deepEqual(h.view.selection, { kind: "target", label: "//app:first" });
});

test("a package selection issues no target reads and drops the in-flight detail", async () => {
  const h = harness();
  await initReady(h);
  await loadSnapshot(h);
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  const detail = pendingRequest(h, "get_target_detail");
  h.session.select({ kind: "package", path: "app" });
  await settle();
  assert.deepEqual(h.view.selection, { kind: "package", path: "app" });
  detail.reply.resolve(targetDetail("//app:first"));
  await settle();
  assert.equal(h.view.targetDetail, null);
  assert.equal(h.view.targetDetailError, null);
  assert.equal(requestCount(h, "get_target_detail"), 1);
  assert.equal(requestCount(h, "get_target_outputs"), 0);
  assert.equal(requestCount(h, "get_target_actions"), 0);
});

test("target actions still load when outputs fails", async () => {
  const h = harness();
  await initReady(h);
  await loadSnapshot(h);
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  pendingRequest(h, "get_target_detail").reply.resolve(targetDetail("//app:first"));
  await settle();
  pendingRequest(h, "get_target_outputs").reply.reject(new Error("cquery failed"));
  await settle();
  const actions = pendingRequest(h, "get_target_actions");
  actions.reply.resolve({ actions: [], truncated: false });
  await settle();
  assert.deepEqual(h.view.targetActions, { actions: [], truncated: false });
  assert.match(h.view.targetOutputsError ?? "", /cquery failed/);
});

test("a failed detail read blocks outputs and actions; reselecting retries it", async () => {
  const h = harness();
  await initReady(h);
  await loadSnapshot(h);
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  pendingRequest(h, "get_target_detail").reply.reject(new Error("detail failed"));
  await settle();
  assert.match(h.view.targetDetailError ?? "", /detail failed/);
  assert.equal(requestCount(h, "get_target_outputs"), 0);
  assert.equal(requestCount(h, "get_target_actions"), 0);
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  const retry = lastPendingRequest(h, "get_target_detail");
  retry.reply.resolve(targetDetail("//app:first"));
  await settle();
  assert.equal(h.view.targetDetail?.target.label, "//app:first");
  assert.equal(h.view.targetDetailError, null);
});

test("history notifications coalesce and owned-state pushes do not refetch", async () => {
  const h = harness();
  await initReady(h);
  await loadHistory(h, []);
  const base = requestCount(h, "list_invocations");
  h.emit("invocations-changed", null);
  await settle();
  assert.equal(requestCount(h, "list_invocations"), base + 1);
  // A burst while that read is in flight collapses into one follow-up.
  h.emit("invocations-changed", null);
  h.emit("invocations-changed", null);
  h.emit("invocations-changed", null);
  await settle();
  assert.equal(requestCount(h, "list_invocations"), base + 1);
  lastPendingRequest(h, "list_invocations").reply.resolve([]);
  await settle();
  assert.equal(requestCount(h, "list_invocations"), base + 2);
  lastPendingRequest(h, "list_invocations").reply.resolve([]);
  await settle();
  assert.equal(requestCount(h, "list_invocations"), base + 2);
  // An owned-state-only app-state push touches neither history nor targets.
  const refreshes = requestCount(h, "refresh");
  h.emit(
    "app-state",
    coreState({
      revision: 2,
      invocations: [owned("q-1", WS, { cmd: "query", console: false })],
    }),
  );
  await settle();
  assert.equal(requestCount(h, "list_invocations"), base + 2);
  assert.equal(requestCount(h, "refresh"), refreshes);
});

test("a history record change refreshes saved readers; retries stay per-reader", async () => {
  const h = harness();
  await initReady(h);
  await loadHistory(h, [COMPLETED_RECORD]);
  h.session.selectInvocation(RECORD.id);
  await settle();
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.equal(h.view.detail?.record.exitCode, 0);
  assert.deepEqual(h.view.profile, PROFILE);
  // The selected record changes: both saved readers refetch, targets do not.
  h.emit("invocations-changed", null);
  await settle();
  await loadHistory(h, [{ ...COMPLETED_RECORD, exitCode: 1 }]);
  await settle();
  const detailRefresh = lastPendingRequest(h, "get_invocation");
  const profileRefresh = lastPendingRequest(h, "get_invocation_profile");
  detailRefresh.reply.reject(new Error("detail refresh failed"));
  profileRefresh.reply.reject(new Error("profile refresh failed"));
  await settle();
  // Cached values stay visible alongside the new errors.
  assert.equal(h.view.detail?.record.exitCode, 0);
  assert.match(h.view.detailError ?? "", /detail refresh failed/);
  assert.deepEqual(h.view.profile, PROFILE);
  assert.match(h.view.profileError ?? "", /profile refresh failed/);
  // An unrelated push does not retry the failed readers.
  h.emit("app-state", coreState({ revision: 2 }));
  await settle();
  assert.equal(requestCount(h, "get_invocation"), 2);
  assert.equal(requestCount(h, "get_invocation_profile"), 2);
  // Explicit retries hit only their own reader.
  h.session.loadDetail();
  await settle();
  assert.equal(requestCount(h, "get_invocation"), 3);
  assert.equal(requestCount(h, "get_invocation_profile"), 2);
  assert.equal(h.view.detailError, null);
  assert.match(h.view.profileError ?? "", /profile refresh failed/);
  h.session.loadProfile();
  await settle();
  assert.equal(requestCount(h, "get_invocation_profile"), 3);
  assert.equal(requestCount(h, "get_invocation"), 3);
  lastPendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  lastPendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.equal(h.view.detailError, null);
  assert.equal(h.view.profileError, null);
});

test("a failed refresh retains the snapshot and selection and exposes the error", async () => {
  const h = harness();
  await initReady(h);
  await loadSnapshot(h);
  h.session.select({ kind: "target", label: "//app:first" });
  await settle();
  // The first detail stays in flight through the failed refresh.
  const firstDetail = pendingRequest(h, "get_target_detail");
  assert.equal(firstDetail.args?.label, "//app:first");
  h.session.refresh();
  await settle();
  lastPendingRequest(h, "refresh").reply.reject(new Error("query exploded"));
  await settle();
  assert.equal(h.view.snapshot?.meta.fetchedAt, 1);
  assert.deepEqual(h.view.selection, { kind: "target", label: "//app:first" });
  assert.match(h.view.snapshotError ?? "", /query exploded/);
  // Refresh reset the target readers up front; nothing refetches while the
  // orphaned chain still holds the busy slot.
  assert.equal(h.view.targetDetail, null);
  // The stale detail's completion frees the chain, which then reads the
  // current selection exactly once.
  firstDetail.reply.resolve(targetDetail("//app:first"));
  await settle();
  assert.equal(requestCount(h, "get_target_detail"), 2);
  assert.equal(h.view.targetDetail, null);
  lastPendingRequest(h, "get_target_detail").reply.resolve(targetDetail("//app:first"));
  await settle();
  // A partial load still installs its snapshot and surfaces the summary; the
  // pending refresh clears the previous error up front.
  h.session.refresh();
  await settle();
  assert.equal(h.view.snapshotError, null);
  lastPendingRequest(h, "refresh").reply.resolve({
    snapshot: { ...SNAPSHOT, meta: { fetchedAt: 2, status: "partial" } },
    error: { exitCode: 9, summary: "skipped pkg" },
  });
  await settle();
  assert.equal(h.view.snapshot?.meta.fetchedAt, 2);
  assert.equal(h.view.snapshotError, "skipped pkg");
});

test("a failed history read surfaces an error and loadInvocations retries", async () => {
  const h = harness();
  await initReady(h);
  pendingRequest(h, "list_invocations").reply.reject(new Error("history unavailable"));
  await settle();
  assert.match(h.view.invocationsError ?? "", /history unavailable/);
  assert.deepEqual(h.view.invocations, []);
  h.session.loadInvocations();
  await settle();
  lastPendingRequest(h, "list_invocations").reply.resolve([COMPLETED_RECORD]);
  await settle();
  assert.deepEqual(h.view.invocations, [COMPLETED_RECORD]);
  assert.equal(h.view.invocationsError, null);
});

test("stale open replies update core but not the selection", async () => {
  const h = harness(
    Promise.resolve(
      coreState({
        revision: 2,
        workspaces: [wsEntry(WS), wsEntry(OTHER), wsEntry(THIRD)],
      }),
    ),
  );
  await initReady(h);
  // Three opens race: A fails stale, B succeeds stale, C is current.
  const openA = h.session.openWorkspace(OTHER, { include: ["//a"] });
  const openB = h.session.openWorkspace(THIRD, { include: ["//b"] });
  const openC = h.session.openWorkspace(OTHER, { include: ["//c"] });
  const requestA = pendingRequest(h, "open_workspace");
  const requests = h.pending.filter((call) => call.command === "open_workspace");
  const requestB = requests[1];
  const requestC = requests[2];
  assert.ok(requestB && requestC);
  // A's late failure resolves false instead of rejecting; nothing else changes.
  requestA.reply.reject(new Error("open failed"));
  assert.equal(await openA, false);
  assert.equal(h.view.state?.workspace, WS);
  // B's late success still applies its newer aggregate but cannot select.
  requestB.reply.resolve({
    workspace: THIRD,
    state: coreState({
      revision: 5,
      workspaces: [wsEntry(WS), wsEntry(OTHER), wsEntry(THIRD)],
    }),
  });
  assert.equal(await openB, false);
  assert.equal(h.view.state?.workspace, WS);
  assert.equal(h.view.core?.revision, 5);
  // C's reply embeds an older aggregate: core stays at 5, yet C's canonical
  // workspace is still selected.
  requestC.reply.resolve({
    workspace: OTHER,
    state: coreState({
      revision: 3,
      workspaces: [wsEntry(WS), wsEntry(OTHER), wsEntry(THIRD)],
    }),
  });
  assert.equal(await openC, true);
  assert.equal(h.view.core?.revision, 5);
  assert.equal(h.view.state?.workspace, OTHER);
  assert.equal(lastPendingRequest(h, "refresh").args?.workspace, OTHER);
});

test("explicit open intent blocks saved-workspace restore, even after failure", async () => {
  const h = harness(
    Promise.resolve(coreState({ revision: 1, workspaces: [], lastWorkspace: null })),
  );
  await initReady(h);
  assert.equal(h.view.state, null);
  const openA = h.session.openWorkspace(OTHER, { include: ["//a"] });
  const openB = h.session.openWorkspace(THIRD, { include: ["//b"] });
  const requests = h.pending.filter((call) => call.command === "open_workspace");
  const requestA = requests[0];
  const requestB = requests[1];
  assert.ok(requestA && requestB);
  const rejection = assert.rejects(openB, /open failed/);
  // A preference push while B is outstanding cannot select a workspace.
  h.emit(
    "app-state",
    coreState({ revision: 2, workspaces: [wsEntry(OTHER)], lastWorkspace: OTHER }),
  );
  await settle();
  assert.equal(h.view.state, null);
  // A's stale success still applies its newer aggregate but cannot select.
  requestA.reply.resolve({
    workspace: OTHER,
    state: coreState({
      revision: 3,
      workspaces: [wsEntry(OTHER)],
      lastWorkspace: OTHER,
    }),
  });
  assert.equal(await openA, false);
  assert.equal(h.view.core?.revision, 3);
  assert.equal(h.view.state, null);
  requestB.reply.reject(new Error("open failed"));
  await rejection;
  // Even after the failed open, later preference pushes cannot select.
  h.emit(
    "app-state",
    coreState({ revision: 4, workspaces: [wsEntry(OTHER)], lastWorkspace: OTHER }),
  );
  await settle();
  assert.equal(h.view.state, null);
  assert.equal(requestCount(h, "list_invocations"), 0);
  assert.equal(requestCount(h, "refresh"), 0);
});

test("a failed workspace open rejects while the current selection remains", async () => {
  const h = harness(
    Promise.resolve(coreState({ revision: 2, workspaces: [wsEntry(WS), wsEntry(OTHER)] })),
  );
  await initReady(h);
  const failed = h.session.openWorkspace(OTHER, { include: ["//..."] });
  const rejection = assert.rejects(failed, /open failed/);
  pendingRequest(h, "open_workspace").reply.reject(new Error("open failed"));
  await rejection;
  assert.equal(h.view.state?.workspace, WS);
  // The form retries the same open; success selects the canonical workspace.
  const retried = h.session.openWorkspace(OTHER, { include: ["//..."] });
  lastPendingRequest(h, "open_workspace").reply.resolve({
    workspace: OTHER,
    state: coreState({
      revision: 3,
      workspaces: [wsEntry(WS), wsEntry(OTHER)],
    }),
  });
  assert.equal(await retried, true);
  assert.equal(h.view.state?.workspace, OTHER);
});

test("a failed initial get_state keeps a pushed state's readiness without error", async () => {
  const initial = deferred<AppState>();
  const h = harness(initial.promise);
  const initialized = h.session.init();
  await h.stateRequested;
  h.emit("app-state", coreState({ revision: 7 }));
  await settle();
  assert.notEqual(h.view.core, null);
  initial.reject(new Error("ipc down"));
  await initialized;
  await settle();
  assert.notEqual(h.view.core, null);
  assert.equal(h.view.core?.revision, 7);
  assert.equal(h.view.error, null);
  assert.equal(h.stateReads(), 1);
  assert.equal(h.listeners.size, 2);
});

test("a stop failure surfaces only while its invocation is displayed", async () => {
  const h = harness(Promise.resolve(coreState({ revision: 2, invocations: [owned("a-run", WS)] })));
  await initReady(h);
  h.session.stop("a-run");
  await settle();
  pendingRequest(h, "stop_invocation").reply.reject(new Error("stop failed"));
  await settle();
  assert.equal(h.view.error, null);
  h.session.selectInvocation("a-run");
  await settle();
  h.session.stop("a-run");
  await settle();
  lastPendingRequest(h, "stop_invocation").reply.reject(new Error("stop failed"));
  await settle();
  assert.match(h.view.error ?? "", /stop failed/);
});
