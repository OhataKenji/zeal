import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { canSend, canStop, createSession, emptyView, type Transport } from "../src/lib/session.ts";
import type { AppState, RunDetail, RunRecord } from "../src/lib/types.ts";

const idle: AppState = {
  workspace: "/workspace",
  bazel: "/bin/bazel",
  ingest: "idle",
  snapshot: null,
  lastError: null,
  running: null,
  stopping: false,
  runNote: null,
};
const record: RunRecord = {
  id: "run-a",
  workspace: "/workspace",
  cmd: "build",
  target: "//:app",
  startedAt: 1,
};
const completed: RunDetail = {
  record: { ...record, finishedAt: 2, exitCode: 0 },
  log: { status: "ok", data: "same\nsame\n" },
  bep: { status: "missing" },
  profile: { status: "missing" },
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

const cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.splice(0).forEach((dispose) => dispose());
});

function harness(initial = Promise.resolve(idle)) {
  let view = emptyView();
  let coreState = idle;
  let initialRequested = false;
  let nextState: Promise<AppState> | undefined;
  const listeners = new Map<string, (payload: unknown) => void>();
  const pending: {
    command: string;
    args?: Record<string, unknown>;
    reply: ReturnType<typeof deferred<unknown>>;
  }[] = [];
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

// Drain promise reactions only; no timers or arbitrary sleeps in race tests.
async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

test("Send waits for listeners, then rejects busy ingestion, a live run and a stopping run", async () => {
  const h = harness();
  assert.equal(canSend(h.view, "//:app"), false);
  await h.session.init();
  assert.deepEqual([...h.listeners.keys()], ["app-state"]);
  assert.equal(canSend(h.view, "//:app"), true);
  h.emit("app-state", { ...idle, ingest: "running" });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", { ...idle, running: record.id });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", { ...idle, running: record.id, stopping: true });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", idle);
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
    ...idle,
    snapshot: null,
    lastError: { exitCode: 1, summary: "query failed" },
  });
  assert.equal(h.view.state?.snapshot, null);
  assert.match(h.view.state!.lastError!.summary, /query failed/);
  assert.equal(canSend(h.view, "//..."), true);
});

test("ingestion completing never starts a run", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...idle, ingest: "running" });
  h.emit("app-state", {
    ...idle,
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
  const call = h.pending.find((item) => item.command === "start_run")!;
  assert.deepEqual(call.args, { cmd: "test", target: "//app:tar;get" });
  call.reply.resolve({ ...record, cmd: "test", target: "//app:tar;get" });
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
  h.emit("app-state", { ...idle, running: record.id });
  assert.equal(canStop(h.view), true);
  const stopping = h.session.stop();
  // No optimistic local flag: the core reports stopping through app-state.
  assert.equal(h.view.state?.stopping, false);
  assert.equal(canStop(h.view), true);
  assert.equal(h.pending.filter((call) => call.command === "stop_run").length, 1);
  h.emit("app-state", { ...idle, running: record.id, stopping: true });
  assert.equal(canStop(h.view), false);
  await h.session.stop();
  assert.equal(h.pending.filter((call) => call.command === "stop_run").length, 1);
  h.pending.find((call) => call.command === "stop_run")!.reply.resolve(undefined);
  await stopping;
});

test("stop failure is surfaced and leaves the run in flight", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...idle, running: record.id });
  const stopping = h.session.stop();
  h.pending
    .find((call) => call.command === "stop_run")!
    .reply.reject(new Error("stop unavailable"));
  await stopping;
  assert.match(h.view.error!, /stop unavailable/);
  assert.equal(h.view.state?.running, record.id);
});

test("full artifacts are read only after the run finishes", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...idle, running: record.id });
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await started;
  assert.equal(h.view.selectedRun, null);
  assert.equal(h.view.console?.id, record.id);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(
    h.pending.some((call) => call.command === "get_run"),
    false,
  );
  h.emit("app-state", idle);
  h.session.selectRun(record.id);
  h.pending
    .filter((call) => call.command === "get_run")
    .at(-1)!
    .reply.resolve(completed);
  await settle();
  assert.deepEqual(h.view.detail, completed);
  assert.equal(h.view.detail?.log.status, "ok");
});

test("run completing before start response immediately selects its All view", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...idle, running: record.id });
  h.emit("app-state", idle);
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await started;
  assert.equal(h.view.state?.running, null);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedRun, record.id);
  assert.equal(h.view.initialRunTab, "All");
  h.pending
    .filter((call) => call.command === "get_run")
    .at(-1)!
    .reply.resolve(completed);
  await settle();
  assert.deepEqual(h.view.detail, completed);
});

test("start response before the running event reconciles state without reading active observations", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.setCore({ ...idle, running: record.id });
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await started;
  assert.equal(h.view.state?.running, record.id);
  assert.equal(
    h.pending.some((call) => call.command === "get_run"),
    false,
  );
  h.emit("app-state", { ...idle, running: record.id });
});

test("completion event during post-start reconciliation wins over its stale running response", async () => {
  const h = harness();
  await h.session.init();
  const reconciled = deferred<AppState>();
  h.nextStateReply(reconciled.promise);
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...idle, running: record.id });
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await settle();
  h.emit("app-state", idle);
  reconciled.resolve({ ...idle, running: record.id });
  await started;
  assert.equal(h.view.state?.running, null);
  h.session.selectRun(record.id);
  h.pending
    .filter((call) => call.command === "get_run")
    .at(-1)!
    .reply.resolve(completed);
  await settle();
  assert.equal(h.view.detail?.record.exitCode, 0);
});

test("failed state reconciliation preserves the started run until a later state event", async () => {
  const h = harness();
  await h.session.init();
  const reconciled = deferred<AppState>();
  h.nextStateReply(reconciled.promise);
  const started = h.session.start("build", "//:app");
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await settle();
  reconciled.reject(new Error("state unavailable"));
  await started;
  assert.match(h.view.error!, /実行は開始済み/);
  assert.equal(canSend(h.view, "//:app"), false);
  assert.equal(h.view.console?.id, record.id);
  assert.equal(
    h.pending.some((call) => call.command === "get_run"),
    false,
  );
  h.session.selectRun(null);
  h.session.selectRun(record.id);
  h.emit("app-state", idle);
  h.pending.find((call) => call.command === "get_run")!.reply.resolve(completed);
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
  h.emit("app-state", { ...idle, workspace: "/new-workspace" });
  bootstrap.resolve(idle);
  await initialized;
  assert.equal(h.view.state?.workspace, "/new-workspace");
});

test("a slow previous run detail cannot replace the selected run", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.session.selectRun("run-b");
  const requests = h.pending.filter((call) => call.command === "get_run");
  requests[1].reply.resolve({ ...completed, record: { ...completed.record, id: "run-b" } });
  await settle();
  requests[0].reply.resolve(completed);
  await settle();
  assert.equal(h.view.selectedRun, "run-b");
  assert.equal(h.view.detail?.record.id, "run-b");
});

test("workspace switch rejects both stale run list and stale detail", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.emit("app-state", { ...idle, workspace: "/other" });
  h.pending.find((call) => call.command === "list_runs")!.reply.resolve([record]);
  h.pending.find((call) => call.command === "get_run")!.reply.resolve(completed);
  await settle();
  assert.deepEqual(h.view.runs, []);
  assert.equal(h.view.detail, null);
  assert.equal(h.view.selectedRun, null);
});

test("target selection cancels an in-flight detail without changing core running state", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.session.selectRun(null);
  h.pending.find((call) => call.command === "get_run")!.reply.resolve(completed);
  await settle();
  assert.equal(h.view.detail, null);
  assert.equal(h.view.detailLoading, false);
});

test("failed spawn is visible and allows another Send", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("test", "//:app");
  h.pending
    .find((call) => call.command === "start_run")!
    .reply.reject(new Error("failed to spawn bazel"));
  await started;
  assert.match(h.view.error!, /failed to spawn/);
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
  h.pending
    .find((call) => call.command === "start_run")!
    .reply.reject(new Error("failed to spawn"));
  await started;
  assert.equal(h.view.detailLoading, true);
  const request = h.pending.find((call) => call.command === "get_run")!;
  assert.deepEqual(request.args, { id: "saved-run" });
  request.reply.resolve({ ...completed, record: { ...completed.record, id: "saved-run" } });
  await settle();
  assert.equal(h.view.detail?.record.id, "saved-run");
});

test("dispose removes listeners and ignores late reads", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.session.dispose();
  h.pending.find((call) => call.command === "get_run")!.reply.resolve(completed);
  await settle();
  assert.deepEqual(h.removed, ["app-state"]);
  assert.equal(h.view.detail, null);
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
        return idle as T;
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
      invoke: async <T>() => idle as T,
    },
    (next) => {
      view = next;
    },
  );
  await session.init();
  assert.equal(canSend(view, "//:app"), false);
  assert.match(view.error!, /listener unavailable/);
});

test("Bazel-busy workspace switch is rejected before calling the backend", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...idle, running: record.id });
  await assert.rejects(h.session.openWorkspace("/other"), /Bazel/);
  assert.equal(
    h.pending.some((call) => call.command === "open_workspace"),
    false,
  );
});

test("refresh is rejected while busy and is never queued", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...idle, running: record.id });
  await h.session.refresh();
  h.emit("app-state", { ...idle, ingest: "running" });
  await h.session.refresh();
  h.emit("app-state", idle);
  assert.equal(
    h.pending.some((call) => call.command === "refresh"),
    false,
  );
  const refreshing = h.session.refresh();
  h.pending.find((call) => call.command === "refresh")!.reply.resolve(undefined);
  await refreshing;
});

test("an unconfirmed exit stays unknown and is not retried automatically", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...idle, running: record.id, stopping: true });
  h.session.selectRun(record.id);
  h.emit("app-state", { ...idle, runNote: "停止を確認できません" });
  h.pending.find((call) => call.command === "get_run")!.reply.resolve({ ...completed, record });
  await settle();
  assert.equal(h.view.detail?.record.exitCode, undefined);
  assert.equal(h.view.state?.runNote, "停止を確認できません");
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
  h.emit("app-state", { ...idle, running: record.id });
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await started;
  h.session.selectRun(null);
  assert.equal(h.view.state?.running, record.id);
  assert.equal(
    h.pending.some((call) => call.command === "stop_run"),
    false,
  );
  h.emit("app-state", idle);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedRun, null);
});

test("process completion selects All immediately, before history finishes loading", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app");
  h.emit("app-state", { ...idle, running: record.id });
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await started;
  assert.equal(h.view.consoleOpen, true);
  h.emit("app-state", idle);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedRun, record.id);
  assert.equal(h.view.initialRunTab, "All");
  assert.equal(h.view.detailLoading, true);
  h.pending
    .filter((call) => call.command === "get_run")
    .at(-1)!
    .reply.resolve(completed);
  await settle();
  assert.deepEqual(h.view.detail, completed);
});
