import { test } from "node:test";
import assert from "node:assert/strict";
import { canSend, createSession, emptyView, type Transport } from "../src/lib/session.ts";
import type { AppState, RunDetail, RunRecord } from "../src/lib/types.ts";

const idle: AppState = {
  workspace: "/workspace",
  bazel: "/bin/bazel",
  ingest: "idle",
  snapshot: null,
  lastError: null,
  running: null,
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

function harness(initial = Promise.resolve(idle)) {
  let view = emptyView();
  const listeners = new Map<string, (payload: unknown) => void>();
  const pending: {
    command: string;
    args?: Record<string, unknown>;
    reply: ReturnType<typeof deferred<unknown>>;
  }[] = [];
  const removed: string[] = [];
  const frames: (() => void)[] = [];
  const stateRequested = deferred<void>();
  const transport: Transport = {
    invoke: <T>(command: string, args?: Record<string, unknown>) => {
      if (command === "get_state") {
        stateRequested.resolve();
        return initial as Promise<T>;
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
  const session = createSession(
    transport,
    (next) => {
      view = next;
    },
    (callback) => {
      frames.push(callback);
      return () => {
        const index = frames.indexOf(callback);
        if (index >= 0) frames.splice(index, 1);
      };
    },
  );
  return {
    session,
    get view() {
      return view;
    },
    listeners,
    pending,
    removed,
    stateRequested: stateRequested.promise,
    emit: (name: string, payload: unknown) => listeners.get(name)?.(payload),
    flush: () => frames.splice(0).forEach((frame) => frame()),
  };
}

// Drain promise reactions only; no timers or arbitrary sleeps in race tests.
async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

test("Send waits for listeners, then rejects busy snapshot/run and duplicate starts", async () => {
  const h = harness();
  assert.equal(canSend(h.view, "//:app"), false);
  await h.session.init();
  assert.deepEqual([...h.listeners.keys()], ["run-log", "app-state"]);
  assert.equal(canSend(h.view, "//:app"), true);
  h.emit("app-state", { ...idle, ingest: "running" });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", { ...idle, running: record.id });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", idle);
  void h.session.start("build", " //:app ");
  void h.session.start("build", "//:app");
  assert.equal(h.pending.filter((call) => call.command === "start_run").length, 1);
  assert.deepEqual(h.pending.find((call) => call.command === "start_run")?.args, {
    cmd: "build",
    target: "//:app",
  });
});

test("early repeated log lines survive start response and completion replaces them with saved log", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...idle, running: record.id });
  h.emit("run-log", { id: record.id, line: "same" });
  h.emit("run-log", { id: record.id, line: "same" });
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await started;
  assert.equal(h.view.selectedRun, record.id);
  assert.equal(h.view.liveLog, "same\nsame");
  assert.equal(
    h.pending.some((call) => call.command === "get_run"),
    false,
  );
  h.emit("app-state", idle);
  h.pending.find((call) => call.command === "get_run")!.reply.resolve(completed);
  await settle();
  assert.deepEqual(h.view.detail, completed);
  assert.equal(h.view.detail?.log.status, "ok");
  h.flush();
  assert.equal(h.view.liveLog, "");
});

test("run completing before start response still opens completed detail", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...idle, running: record.id });
  h.emit("run-log", { id: record.id, line: "finished" });
  h.emit("app-state", idle);
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await started;
  assert.equal(h.view.state?.running, null);
  assert.equal(h.view.selectedRun, record.id);
  h.pending.find((call) => call.command === "get_run")!.reply.resolve(completed);
  await settle();
  assert.equal(h.view.detail?.record.exitCode, 0);
});

test("a run finishing offscreen releases its temporary live output", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...idle, running: record.id });
  h.emit("run-log", { id: record.id, line: "temporary live output" });
  h.pending.find((call) => call.command === "start_run")!.reply.resolve(record);
  await started;
  assert.equal(h.view.liveLog, "temporary live output");
  h.session.selectRun(null);
  h.emit("app-state", idle);
  h.session.selectRun(record.id);
  assert.equal(h.view.liveLog, "");
  assert.equal(h.view.liveFromStart, false);
  assert.equal(h.view.detailLoading, true);
  h.pending.find((call) => call.command === "get_run")!.reply.resolve(completed);
  await settle();
  assert.deepEqual(h.view.detail, completed);
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

test("dispose removes listeners and ignores late reads", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectRun("run-a");
  h.session.dispose();
  h.pending.find((call) => call.command === "get_run")!.reply.resolve(completed);
  await settle();
  assert.deepEqual(h.removed, ["run-log", "app-state"]);
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
    () => () => {},
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

test("partial subscription failure cleans the first listener and leaves Send disabled", async () => {
  let view = emptyView();
  let removed = false;
  const session = createSession(
    {
      listen: async (name) => {
        if (name === "app-state") throw new Error("listener unavailable");
        return () => {
          removed = true;
        };
      },
      invoke: async <T>() => idle as T,
    },
    (next) => {
      view = next;
    },
    () => () => {},
  );
  await session.init();
  assert.equal(removed, true);
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
