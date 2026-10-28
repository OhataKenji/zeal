import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { canSend, canStop, createSession, emptyView, type Transport } from "../src/lib/session.ts";
import type { AppState, Profile, RunDetail, RunRecord, Section } from "../src/lib/types.ts";

const IDLE: AppState = {
  workspace: "/workspace",
  bazel: "/bin/bazel",
  ingest: "idle",
  snapshot: null,
  lastError: null,
  running: null,
  stopping: false,
  runNote: null,
};
const RECORD: RunRecord = {
  id: "run-a",
  workspace: "/workspace",
  cmd: "build",
  target: "//:app",
  startedAt: 1,
};
const COMPLETED: RunDetail = {
  record: { ...RECORD, finishedAt: 2, exitCode: 0 },
  log: { status: "ok", data: "same\nsame\n" },
  bep: { status: "missing" },
};
const PROFILE: Section<Profile> = { status: "ok", data: { phases: [], lanes: [] } };

function deferred<T>() {
  // Promise constructors assign these callbacks before `promise` is returned.
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

function harness(initial = Promise.resolve(IDLE)) {
  let view = emptyView();
  let coreState = IDLE;
  let initialRequested = false;
  let nextState: Promise<AppState> | undefined;
  const listeners = new Map<string, (payload: unknown) => void>();
  const pending: Array<{
    command: string;
    args?: Record<string, unknown>;
    reply: ReturnType<typeof deferred<unknown>>;
  }> = [];
  const removed: string[] = [];
  const stateRequested = deferred<void>();
  const transport: Transport = {
    invoke: <T>(command: string, args?: Record<string, unknown>) => {
      if (command === "get_state") {
        stateRequested.resolve();
        if (!initialRequested) {
          initialRequested = true;
          return initial as Promise<T>;
        }
        const response = nextState ?? Promise.resolve(coreState);
        nextState = undefined;
        return response as Promise<T>;
      }
      const reply = deferred<unknown>();
      pending.push({ command, args, reply });
      return reply.promise as Promise<T>;
    },
    listen: async <T>(name: string, callback: (payload: T) => void) => {
      listeners.set(name, callback as (payload: unknown) => void);
      return () => {
        listeners.delete(name);
        removed.push(name);
      };
    },
  };
  const session = createSession(transport, (next) => {
    view = next;
  });
  cleanup.push(session.dispose);
  return {
    session,
    get view() {
      return view;
    },
    listeners,
    pending,
    removed,
    stateRequested: stateRequested.promise,
    emit: (name: string, payload: unknown) => {
      if (name === "app-state") coreState = payload as AppState;
      listeners.get(name)?.(payload);
    },
    setCore: (state: AppState) => {
      coreState = state;
    },
    nextStateReply: (reply: Promise<AppState>) => {
      nextState = reply;
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

// Drain promise reactions only; no timers or arbitrary sleeps in race tests.
async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

test("logs and results arrive while profile parsing is still pending", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun(RECORD.id);
  const detailRequest = pendingRequest(h, "get_run");
  const profileRequest = pendingRequest(h, "get_run_profile");
  assert.deepEqual(profileRequest.args, { id: RECORD.id });
  detailRequest.reply.resolve(COMPLETED);
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
  assert.equal(h.view.detailLoading, false);
  assert.equal(h.view.profileLoading, true);
  assert.equal(h.view.profile, null);
  profileRequest.reply.resolve(PROFILE);
  await settle();
  assert.deepEqual(h.view.profile, PROFILE);
  assert.equal(h.view.profileLoading, false);
});

test("timing can arrive before logs without clearing either response", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun(RECORD.id);
  pendingRequest(h, "get_run_profile").reply.resolve(PROFILE);
  await settle();
  assert.deepEqual(h.view.profile, PROFILE);
  assert.equal(h.view.detailLoading, true);
  pendingRequest(h, "get_run").reply.resolve(COMPLETED);
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
  assert.deepEqual(h.view.profile, PROFILE);
});

test("a profile failure leaves logs visible and retries only timing", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun(RECORD.id);
  pendingRequest(h, "get_run").reply.resolve(COMPLETED);
  pendingRequest(h, "get_run_profile").reply.reject(new Error("profile worker busy"));
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
  assert.equal(h.view.detailError, null);
  assert.match(h.view.profileError ?? "", /profile worker busy/);
  const retry = h.session.loadProfile();
  lastPendingRequest(h, "get_run_profile").reply.resolve(PROFILE);
  await retry;
  assert.equal(h.pending.filter((call) => call.command === "get_run").length, 1);
  assert.deepEqual(h.view.profile, PROFILE);
  assert.equal(h.view.profileError, null);
});

test("a log request failure leaves timing visible and retries only the summary", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun(RECORD.id);
  pendingRequest(h, "get_run").reply.reject(new Error("record unavailable"));
  pendingRequest(h, "get_run_profile").reply.resolve(PROFILE);
  await settle();
  assert.deepEqual(h.view.profile, PROFILE);
  assert.match(h.view.detailError ?? "", /record unavailable/);
  const retry = h.session.loadDetail();
  lastPendingRequest(h, "get_run").reply.resolve(COMPLETED);
  await retry;
  assert.deepEqual(h.view.detail, COMPLETED);
  assert.equal(h.view.detailError, null);
  assert.equal(h.pending.filter((call) => call.command === "get_run_profile").length, 1);
});

test("a late profile response or error cannot replace a newer selection", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.session.selectRun("run-b");
  h.session.selectRun("run-c");
  const requests = h.pending.filter((call) => call.command === "get_run_profile");
  requests[2].reply.resolve(PROFILE);
  await settle();
  requests[0].reply.resolve({ status: "missing" });
  requests[1].reply.reject(new Error("stale error"));
  await settle();
  assert.equal(h.view.selectedRun, "run-c");
  assert.deepEqual(h.view.profile, PROFILE);
  assert.equal(h.view.profileError, null);
  assert.equal(h.view.profileLoading, false);
});

test("Send waits for listeners, then rejects busy ingestion, a live run and a stopping run", async () => {
  const h = harness();
  assert.equal(canSend(h.view, "//:app"), false);
  await h.session.init();
  assert.deepEqual([...h.listeners.keys()], ["app-state"]);
  assert.equal(canSend(h.view, "//:app"), true);
  h.emit("app-state", { ...IDLE, ingest: "running" });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", { ...IDLE, running: RECORD.id, stopping: true });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", IDLE);
  assert.equal(canSend(h.view, "//:app"), true);
});

test("Send rejects invalid targets and accepts patterns", async () => {
  const h = harness();
  await h.session.init();
  assert.equal(canSend(h.view, ""), false);
  assert.equal(canSend(h.view, " "), false);
  assert.equal(canSend(h.view, "a\0b"), false);
  assert.equal(canSend(h.view, "-//a:b"), false);
  assert.equal(canSend(h.view, "//..."), true);
  assert.equal(canSend(h.view, ":all"), true);
});

test("Send does not depend on a snapshot and stays usable after a failed ingestion", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", {
    ...IDLE,
    snapshot: null,
    lastError: { exitCode: 1, summary: "query failed" },
  });
  assert.equal(h.view.state?.snapshot, null);
  const lastError = h.view.state?.lastError;
  assert.ok(lastError);
  assert.match(lastError.summary, /query failed/);
  assert.equal(canSend(h.view, "//..."), true);
});

test("ingestion completing never starts a run", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, ingest: "running" });
  h.emit("app-state", {
    ...IDLE,
    snapshot: { meta: { fetchedAt: 1, status: "ok" }, targets: [], edges: [], files: [] },
  });
  await settle();
  assert.equal(
    h.pending.some((call) => call.command === "start_run"),
    false,
  );
  assert.equal(canSend(h.view, "//:app"), true);
});

test("Send passes the target through unchanged and ignores whitespace and duplicate starts", async () => {
  const h = harness();
  await h.session.init();
  void h.session.start("build", " //:app ");
  assert.equal(
    h.pending.some((call) => call.command === "start_run"),
    false,
  );
  void h.session.start("build", "//:app");
  void h.session.start("build", "//:app");
  const starts = h.pending.filter((call) => call.command === "start_run");
  assert.equal(starts.length, 1);
  assert.deepEqual(starts[0].args, { cmd: "build", target: "//:app" });
});

test("start records the selected cmd and the target string unchanged", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("test", "//app:tar;get");
  const call = pendingRequest(h, "start_run");
  assert.deepEqual(call.args, { cmd: "test", target: "//app:tar;get" });
  call.reply.resolve({ ...RECORD, cmd: "test", target: "//app:tar;get" });
  await started;
});

test("stop is offered only while a run is live and not already stopping", async () => {
  const h = harness();
  assert.equal(canStop(h.view), false);
  await h.session.init();
  assert.equal(canStop(h.view), false);
  await h.session.stop();
  assert.equal(
    h.pending.some((call) => call.command === "stop_run"),
    false,
  );
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  assert.equal(canStop(h.view), true);
  const stopping = h.session.stop();
  // No optimistic local flag: the core reports stopping through app-state.
  assert.equal(h.view.state?.stopping, false);
  assert.equal(canStop(h.view), true);
  assert.equal(h.pending.filter((call) => call.command === "stop_run").length, 1);
  h.emit("app-state", { ...IDLE, running: RECORD.id, stopping: true });
  assert.equal(canStop(h.view), false);
  await h.session.stop();
  assert.equal(h.pending.filter((call) => call.command === "stop_run").length, 1);
  pendingRequest(h, "stop_run").reply.resolve(undefined);
  await stopping;
});

test("stop failure is surfaced and leaves the run in flight", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  const stopping = h.session.stop();
  pendingRequest(h, "stop_run").reply.reject(new Error("stop unavailable"));
  await stopping;
  assert.match(h.view.error ?? "", /stop unavailable/);
  assert.equal(h.view.state?.running, RECORD.id);
});

test("full artifacts are read only after the run finishes", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  pendingRequest(h, "start_run").reply.resolve(RECORD);
  await started;
  assert.equal(h.view.selectedRun, null);
  assert.equal(h.view.console?.id, RECORD.id);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(
    h.pending.some((call) => call.command === "get_run" || call.command === "get_run_profile"),
    false,
  );
  h.emit("app-state", IDLE);
  h.session.selectRun(RECORD.id);
  assert.equal(h.view.initialRunTab, "All");
  lastPendingRequest(h, "get_run").reply.resolve(COMPLETED);
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
  assert.equal(h.view.detail?.log.status, "ok");
});

test("run completing before start response immediately selects its All view", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  h.emit("app-state", IDLE);
  pendingRequest(h, "start_run").reply.resolve(RECORD);
  await started;
  assert.equal(h.view.state?.running, null);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedRun, RECORD.id);
  assert.equal(h.view.initialRunTab, "All");
  lastPendingRequest(h, "get_run").reply.resolve(COMPLETED);
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
});

test("start response before the running event reconciles state without reading active observations", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.setCore({ ...IDLE, running: RECORD.id });
  pendingRequest(h, "start_run").reply.resolve(RECORD);
  await started;
  assert.equal(h.view.state?.running, RECORD.id);
  assert.equal(
    h.pending.some((call) => call.command === "get_run"),
    false,
  );
  h.emit("app-state", { ...IDLE, running: RECORD.id });
});

test("completion event during post-start reconciliation wins over its stale running response", async () => {
  const h = harness();
  await h.session.init();
  const reconciled = deferred<AppState>();
  h.nextStateReply(reconciled.promise);
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  pendingRequest(h, "start_run").reply.resolve(RECORD);
  await settle();
  h.emit("app-state", IDLE);
  reconciled.resolve({ ...IDLE, running: RECORD.id });
  await started;
  assert.equal(h.view.state?.running, null);
  h.session.selectRun(RECORD.id);
  lastPendingRequest(h, "get_run").reply.resolve(COMPLETED);
  await settle();
  assert.equal(h.view.detail?.record.exitCode, 0);
});

test("failed state reconciliation preserves the started run until a later state event", async () => {
  const h = harness();
  await h.session.init();
  const reconciled = deferred<AppState>();
  h.nextStateReply(reconciled.promise);
  const started = h.session.start("build", "//:app");
  pendingRequest(h, "start_run").reply.resolve(RECORD);
  await settle();
  reconciled.reject(new Error("state unavailable"));
  await started;
  assert.match(h.view.error ?? "", /run started/);
  assert.equal(canSend(h.view, "//:app"), false);
  assert.equal(h.view.console?.id, RECORD.id);
  assert.equal(
    h.pending.some((call) => call.command === "get_run"),
    false,
  );
  h.session.selectRun(null);
  h.session.selectRun(RECORD.id);
  h.emit("app-state", IDLE);
  pendingRequest(h, "get_run").reply.resolve(COMPLETED);
  await settle();
  assert.equal(h.view.error, null);
  assert.equal(h.view.detail?.record.exitCode, 0);
  assert.equal(canSend(h.view, "//:app"), true);
});

test("late bootstrap state cannot overwrite a newer workspace event", async () => {
  const bootstrap = deferred<AppState>();
  const h = harness(bootstrap.promise);
  const initialized = h.session.init();
  await h.stateRequested;
  h.emit("app-state", { ...IDLE, workspace: "/new-workspace" });
  bootstrap.resolve(IDLE);
  await initialized;
  assert.equal(h.view.state?.workspace, "/new-workspace");
});

test("a slow previous run detail cannot replace the selected run", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.session.selectRun("run-b");
  const requests = h.pending.filter((call) => call.command === "get_run");
  requests[1].reply.resolve({ ...COMPLETED, record: { ...COMPLETED.record, id: "run-b" } });
  await settle();
  requests[0].reply.resolve(COMPLETED);
  await settle();
  assert.equal(h.view.selectedRun, "run-b");
  assert.equal(h.view.detail?.record.id, "run-b");
});

test("workspace switch rejects both stale run list and stale detail", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.emit("app-state", { ...IDLE, workspace: "/other" });
  pendingRequest(h, "list_runs").reply.resolve([RECORD]);
  pendingRequest(h, "get_run").reply.resolve(COMPLETED);
  pendingRequest(h, "get_run_profile").reply.resolve(PROFILE);
  await settle();
  assert.deepEqual(h.view.runs, []);
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
  assert.equal(h.view.profileLoading, false);
  assert.equal(h.view.selectedRun, null);
});

test("target selection cancels an in-flight detail without changing core running state", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.session.selectRun(null);
  pendingRequest(h, "get_run").reply.resolve(COMPLETED);
  pendingRequest(h, "get_run_profile").reply.resolve(PROFILE);
  await settle();
  assert.equal(h.view.detail, null);
  assert.equal(h.view.detailLoading, false);
  assert.equal(h.view.profile, null);
  assert.equal(h.view.profileLoading, false);
});

test("failed spawn is visible and allows another Send", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("test", "//:app");
  pendingRequest(h, "start_run").reply.reject(new Error("failed to spawn bazel"));
  await started;
  assert.match(h.view.error ?? "", /failed to spawn/);
  assert.equal(canSend(h.view, "//:app"), true);
});

test("failed spawn resumes a saved run detail selected while start was pending", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.session.selectRun("saved-run");
  assert.equal(
    h.pending.some((call) => call.command === "get_run"),
    false,
  );
  pendingRequest(h, "start_run").reply.reject(new Error("failed to spawn"));
  await started;
  assert.equal(h.view.detailLoading, true);
  const request = pendingRequest(h, "get_run");
  assert.deepEqual(request.args, { id: "saved-run" });
  request.reply.resolve({ ...COMPLETED, record: { ...COMPLETED.record, id: "saved-run" } });
  await settle();
  assert.equal(h.view.detail?.record.id, "saved-run");
});

test("dispose removes listeners and ignores late reads", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.session.dispose();
  pendingRequest(h, "get_run").reply.resolve(COMPLETED);
  pendingRequest(h, "get_run_profile").reply.resolve(PROFILE);
  await settle();
  assert.deepEqual(h.removed, ["app-state"]);
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
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
        return IDLE as T;
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

test("subscription failure leaves Send disabled", async () => {
  let view = emptyView();
  const session = createSession(
    {
      listen: async () => {
        throw new Error("listener unavailable");
      },
      invoke: async <T>() => IDLE as T,
    },
    (next) => {
      view = next;
    },
  );
  await session.init();
  assert.equal(canSend(view, "//:app"), false);
  assert.match(view.error ?? "", /listener unavailable/);
});

test("Bazel-busy workspace switch is rejected before calling the backend", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  await assert.rejects(h.session.openWorkspace("/other"), /Bazel/);
  assert.equal(
    h.pending.some((call) => call.command === "open_workspace"),
    false,
  );
});

test("refresh is rejected while busy and is never queued", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  await h.session.refresh();
  h.emit("app-state", { ...IDLE, ingest: "running" });
  await h.session.refresh();
  h.emit("app-state", IDLE);
  assert.equal(
    h.pending.some((call) => call.command === "refresh"),
    false,
  );
  const refreshing = h.session.refresh();
  pendingRequest(h, "refresh").reply.resolve(undefined);
  await refreshing;
});

test("an unconfirmed exit stays unknown and is not retried automatically", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, running: RECORD.id, stopping: true });
  h.session.selectRun(RECORD.id);
  h.emit("app-state", { ...IDLE, runNote: "Could not confirm the stop" });
  pendingRequest(h, "get_run").reply.resolve({ ...COMPLETED, record: RECORD });
  await settle();
  assert.equal(h.view.detail?.record.exitCode, undefined);
  assert.equal(h.view.state?.runNote, "Could not confirm the stop");
  assert.equal(
    h.pending.some((call) => call.command === "start_run" || call.command === "refresh"),
    false,
  );
  assert.equal(canSend(h.view, "//:app"), true);
});

test("leaving Console keeps the process and prevents automatic navigation", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app");
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  pendingRequest(h, "start_run").reply.resolve(RECORD);
  await started;
  h.session.selectRun(null);
  assert.equal(h.view.state?.running, RECORD.id);
  assert.equal(
    h.pending.some((call) => call.command === "stop_run"),
    false,
  );
  h.emit("app-state", IDLE);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedRun, null);
});

test("process completion selects All immediately, before history finishes loading", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app");
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  pendingRequest(h, "start_run").reply.resolve(RECORD);
  await started;
  assert.equal(h.view.consoleOpen, true);
  h.emit("app-state", IDLE);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedRun, RECORD.id);
  assert.equal(h.view.initialRunTab, "All");
  assert.equal(h.view.detailLoading, true);
  lastPendingRequest(h, "get_run").reply.resolve(COMPLETED);
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
});

test("the live run stays in the list so its row can reopen the Console", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  pendingRequest(h, "list_runs").reply.resolve([RECORD]);
  await settle();
  assert.deepEqual(h.view.runs, [RECORD]);
});

test("openConsole returns to a Console left while its run is in flight", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app");
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  pendingRequest(h, "start_run").reply.resolve(RECORD);
  await started;
  h.session.selectRun(null);
  assert.equal(h.view.consoleOpen, false);
  h.session.openConsole(RECORD);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(h.view.selectedRun, null);
  assert.equal(h.view.console?.id, RECORD.id);
  assert.equal(
    h.pending.some((call) => call.command === "stop_run" || call.command === "get_run"),
    false,
  );
});

test("openConsole restores the Console for a run already in flight at connect", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  assert.equal(h.view.console, null);
  h.session.openConsole(RECORD);
  assert.equal(h.view.console?.id, RECORD.id);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(h.view.selectedRun, null);
});

test("openConsole cancels an in-flight run detail read", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  h.session.selectRun("run-b");
  h.session.openConsole(RECORD);
  pendingRequest(h, "get_run").reply.resolve(COMPLETED);
  pendingRequest(h, "get_run_profile").reply.resolve(PROFILE);
  await settle();
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
  assert.equal(h.view.profileLoading, false);
  assert.equal(h.view.selectedRun, null);
  assert.equal(h.view.consoleOpen, true);
});

test("openConsole on a finished run lands on its detail view", async () => {
  const h = harness();
  await h.session.init();
  h.session.openConsole(RECORD);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedRun, RECORD.id);
  assert.equal(h.view.initialRunTab, "All");
  pendingRequest(h, "get_run").reply.resolve(COMPLETED);
  await settle();
  assert.equal(h.view.detail?.record.id, RECORD.id);
});

test("a run finishing while its reopened Console is shown selects its All view", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app");
  h.emit("app-state", { ...IDLE, running: RECORD.id });
  pendingRequest(h, "start_run").reply.resolve(RECORD);
  await started;
  h.session.selectRun(null);
  h.session.openConsole(RECORD);
  assert.equal(h.view.consoleOpen, true);
  h.emit("app-state", IDLE);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedRun, RECORD.id);
  assert.equal(h.view.initialRunTab, "All");
  assert.equal(h.view.detailLoading, true);
  lastPendingRequest(h, "get_run").reply.resolve(COMPLETED);
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
});
