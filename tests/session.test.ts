import { test } from "node:test";
import assert from "node:assert/strict";
import { createSession, emptyView, type Transport } from "../src/lib/session.ts";
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
  const session = createSession(transport, (next) => {
    view = next;
  });
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
      listeners.get(name)?.(payload);
    },
  };
}

// Drain promise reactions only; no timers or arbitrary sleeps in race tests.
async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

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
