import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { canSend, canStop, createSession, emptyView, type Transport } from "../src/lib/session.ts";
import type {
  AppState,
  InvocationDetail,
  InvocationRecord,
  Profile,
  Section,
  TargetIndex,
} from "../src/lib/types.ts";

const IDLE: AppState = {
  besReady: false,
  scope: { include: ["//app/..."] },
  workspace: "/workspace",
  bazel: "/bin/bazel",
  ingest: "idle",
  snapshotFetchedAt: null,
  lastError: null,
  activeInvocation: null,
  stopping: false,
  invocationNote: null,
  query: "idle",
  detailQuery: "idle",
};
const SNAPSHOT = {
  meta: { fetchedAt: 1, status: "ok" as const },
  targets: [],
};
const TARGETS = {
  ...SNAPSHOT,
  targets: ["//app:first", "//app:second"].map((label) => ({
    label,
    pkg: "app",
    name: label.split(":")[1],
    ruleClass: "cc_library",
    location: null,
  })),
};
const TARGET_DETAIL = {
  target: { ...TARGETS.targets[0], location: { file: "/workspace/app/BUILD", line: 3 } },
  edges: [{ from: "//app:second", to: "//app:first", attr: "deps" }],
  files: [{ owner: "//app:first", path: "app/first.cc", role: "srcs" }],
  status: "ok" as const,
};
const RECORD: InvocationRecord = {
  id: "run-a",
  workspace: "/workspace",
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

function pendingRequests(h: ReturnType<typeof harness>, command: string) {
  return h.pending.filter((call) => call.command === command);
}

function requestCount(h: ReturnType<typeof harness>, command: string) {
  return pendingRequests(h, command).length;
}

function noPending(h: ReturnType<typeof harness>, ...commands: string[]) {
  assert.deepEqual(
    h.pending.filter((call) => commands.includes(call.command)).map((call) => call.command),
    [],
  );
}

// The common arrange: an index-ready event, then resolve its fetch.
async function emitSnapshot(
  h: ReturnType<typeof harness>,
  snapshot: TargetIndex,
  state: Partial<AppState> = {},
) {
  h.emit("app-state", { ...IDLE, ...state, snapshotFetchedAt: snapshot.meta.fetchedAt });
  lastPendingRequest(h, "get_snapshot").reply.resolve(snapshot);
  await settle();
}

// The common arrange: list a record, then select it.
async function selectRecord(h: ReturnType<typeof harness>, records: InvocationRecord[] = [RECORD]) {
  pendingRequest(h, "list_invocations").reply.resolve(records);
  await settle();
  h.session.selectInvocation(RECORD.id);
}

// Drain promise reactions only; no timers or arbitrary sleeps in race tests.
async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

test("logs and results arrive while profile parsing is still pending", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectInvocation(RECORD.id);
  const detailRequest = pendingRequest(h, "get_invocation");
  const profileRequest = pendingRequest(h, "get_invocation_profile");
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

test("history notification refreshes terminal details and missing timing after completion", async () => {
  const h = harness();
  await h.session.init();
  await selectRecord(h);
  const earlier = { ...COMPLETED, record: RECORD };
  pendingRequest(h, "get_invocation").reply.resolve(earlier);
  pendingRequest(h, "get_invocation_profile").reply.resolve({ status: "missing" });
  await settle();

  h.emit("invocations-changed", undefined);
  lastPendingRequest(h, "list_invocations").reply.resolve([COMPLETED.record]);
  await settle();
  assert.equal(requestCount(h, "get_invocation"), 2);
  assert.equal(h.view.detail, earlier);
  assert.equal(h.view.detailLoading, true);
  assert.equal(h.view.selectedInvocation, RECORD.id);
  assert.equal(requestCount(h, "get_invocation_profile"), 2);
  assert.equal(h.view.profileLoading, true);
  lastPendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  lastPendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.equal(h.view.detail, COMPLETED);
  assert.equal(h.view.profile, PROFILE);
});

test("history notifications during a list read trigger one follow-up read", async () => {
  const h = harness();
  await h.session.init();
  h.emit("invocations-changed", undefined);
  h.emit("invocations-changed", undefined);
  assert.equal(requestCount(h, "list_invocations"), 1);
  pendingRequest(h, "list_invocations").reply.resolve([]);
  await settle();
  assert.equal(requestCount(h, "list_invocations"), 2);
  lastPendingRequest(h, "list_invocations").reply.resolve([RECORD]);
  await settle();
  assert.deepEqual(h.view.invocations, [RECORD]);
  assert.equal(requestCount(h, "list_invocations"), 2);
});

test("a history notification still refreshes after an in-flight read fails", async () => {
  const h = harness();
  await h.session.init();
  h.emit("invocations-changed", undefined);
  pendingRequest(h, "list_invocations").reply.reject(new Error("read failed"));
  await settle();
  assert.equal(requestCount(h, "list_invocations"), 2);
  lastPendingRequest(h, "list_invocations").reply.resolve([RECORD]);
  await settle();
  assert.deepEqual(h.view.invocations, [RECORD]);
  assert.equal(h.view.invocationsError, null);
});

test("the next unchanged list refresh recovers completion during a detail read", async () => {
  const h = harness();
  await h.session.init();
  await selectRecord(h);
  const first = h.session.loadInvocations();
  lastPendingRequest(h, "list_invocations").reply.resolve([COMPLETED.record]);
  await first;
  assert.equal(requestCount(h, "get_invocation"), 1);
  pendingRequest(h, "get_invocation").reply.resolve({ ...COMPLETED, record: RECORD });
  await settle();

  const next = h.session.loadInvocations();
  lastPendingRequest(h, "list_invocations").reply.resolve([COMPLETED.record]);
  await next;
  assert.equal(requestCount(h, "get_invocation"), 2);
  lastPendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  await settle();
  const stable = h.session.loadInvocations();
  lastPendingRequest(h, "list_invocations").reply.resolve([COMPLETED.record]);
  await stable;
  assert.equal(requestCount(h, "get_invocation"), 2);
});

test("a failed automatic detail refresh waits for explicit Retry", async () => {
  const h = harness();
  await h.session.init();
  await selectRecord(h);
  const earlier = { ...COMPLETED, record: RECORD };
  pendingRequest(h, "get_invocation").reply.resolve(earlier);
  await settle();
  const first = h.session.loadInvocations();
  lastPendingRequest(h, "list_invocations").reply.resolve([COMPLETED.record]);
  await first;
  lastPendingRequest(h, "get_invocation").reply.reject(new Error("record unavailable"));
  await settle();
  const next = h.session.loadInvocations();
  lastPendingRequest(h, "list_invocations").reply.resolve([COMPLETED.record]);
  await next;
  assert.equal(requestCount(h, "get_invocation"), 2);
  assert.equal(h.view.detail, earlier);
  assert.match(h.view.detailError ?? "", /record unavailable/);
  const retry = h.session.loadDetail();
  lastPendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  await retry;
  assert.equal(h.view.detailError, null);
});

test("another invocation changing or completing does not reload the viewed invocation or profile", async () => {
  const h = harness();
  await h.session.init();
  await selectRecord(h, [COMPLETED.record]);
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  h.emit("app-state", { ...IDLE, activeInvocation: "another-run" });
  lastPendingRequest(h, "list_invocations").reply.resolve([
    { ...COMPLETED.record, id: "another-run" },
    COMPLETED.record,
  ]);
  await settle();
  h.emit("app-state", IDLE);
  assert.equal(requestCount(h, "get_invocation"), 1);
  assert.equal(requestCount(h, "get_invocation_profile"), 1);
  assert.equal(h.view.profile, PROFILE);
});

test("a list response after disposal cannot start a detail refresh", async () => {
  const h = harness();
  await h.session.init();
  await selectRecord(h);
  pendingRequest(h, "get_invocation").reply.resolve({ ...COMPLETED, record: RECORD });
  await settle();
  const poll = h.session.loadInvocations();
  h.session.dispose();
  lastPendingRequest(h, "list_invocations").reply.resolve([COMPLETED.record]);
  await poll;
  assert.equal(requestCount(h, "get_invocation"), 1);
});

test("timing can arrive before logs without clearing either response", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectInvocation(RECORD.id);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.deepEqual(h.view.profile, PROFILE);
  assert.equal(h.view.detailLoading, true);
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
  assert.deepEqual(h.view.profile, PROFILE);
});

test("a profile failure leaves logs visible and retries only timing", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectInvocation(RECORD.id);
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.reject(new Error("profile worker busy"));
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
  assert.equal(h.view.detailError, null);
  assert.match(h.view.profileError ?? "", /profile worker busy/);
  const retry = h.session.loadProfile();
  lastPendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await retry;
  assert.equal(requestCount(h, "get_invocation"), 1);
  assert.deepEqual(h.view.profile, PROFILE);
  assert.equal(h.view.profileError, null);
});

test("a log request failure leaves timing visible and retries only the summary", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectInvocation(RECORD.id);
  pendingRequest(h, "get_invocation").reply.reject(new Error("record unavailable"));
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.deepEqual(h.view.profile, PROFILE);
  assert.match(h.view.detailError ?? "", /record unavailable/);
  const retry = h.session.loadDetail();
  lastPendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  await retry;
  assert.deepEqual(h.view.detail, COMPLETED);
  assert.equal(h.view.detailError, null);
  assert.equal(requestCount(h, "get_invocation_profile"), 1);
});

test("a late profile response or error cannot replace a newer selection", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectInvocation("run-a");
  h.session.selectInvocation("run-b");
  h.session.selectInvocation("run-c");
  const requests = pendingRequests(h, "get_invocation_profile");
  requests[2].reply.resolve(PROFILE);
  await settle();
  requests[0].reply.resolve({ status: "missing" });
  requests[1].reply.reject(new Error("stale error"));
  await settle();
  assert.equal(h.view.selectedInvocation, "run-c");
  assert.deepEqual(h.view.profile, PROFILE);
  assert.equal(h.view.profileError, null);
  assert.equal(h.view.profileLoading, false);
});

test("Send waits for listeners and rejects every active Bazel operation", async () => {
  const h = harness();
  assert.equal(canSend(h.view, "//:app"), false);
  await h.session.init();
  assert.deepEqual([...h.listeners.keys()], ["invocations-changed", "app-state"]);
  assert.equal(canSend(h.view, "//:app"), true);
  h.emit("app-state", { ...IDLE, ingest: "running" });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id, stopping: true });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", { ...IDLE, query: "running" });
  assert.equal(canSend(h.view, "//:app"), false);
  h.emit("app-state", { ...IDLE, detailQuery: "running" });
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
    snapshotFetchedAt: null,
    lastError: { exitCode: 1, summary: "query failed" },
  });
  const lastError = h.view.state?.lastError;
  assert.ok(lastError);
  assert.match(lastError.summary, /query failed/);
  assert.equal(canSend(h.view, "//..."), true);
});

test("ingestion completing never starts an invocation", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, ingest: "running" });
  await emitSnapshot(h, SNAPSHOT);
  assert.deepEqual(h.view.snapshot, SNAPSHOT);
  noPending(h, "start_invocation");
  assert.equal(canSend(h.view, "//:app"), true);
});

test("snapshot data is fetched once per snapshotFetchedAt change, not per state event", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, SNAPSHOT);
  h.emit("app-state", {
    ...IDLE,
    snapshotFetchedAt: SNAPSHOT.meta.fetchedAt,
    activeInvocation: "run-a",
  });
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: SNAPSHOT.meta.fetchedAt });
  await settle();
  assert.equal(requestCount(h, "get_snapshot"), 1);
});

test("events during a snapshot fetch do not duplicate the request", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: SNAPSHOT.meta.fetchedAt });
  assert.equal(h.view.snapshotLoading, true);
  h.emit("app-state", {
    ...IDLE,
    snapshotFetchedAt: SNAPSHOT.meta.fetchedAt,
    activeInvocation: "run-a",
  });
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: SNAPSHOT.meta.fetchedAt });
  assert.equal(requestCount(h, "get_snapshot"), 1);
  pendingRequest(h, "get_snapshot").reply.resolve(SNAPSHOT);
  await settle();
  assert.deepEqual(h.view.snapshot, SNAPSHOT);
  assert.equal(h.view.snapshotLoading, false);
});

test("a stale snapshot fetch cannot replace the current snapshot", async () => {
  const h = harness();
  await h.session.init();
  const newer = {
    ...SNAPSHOT,
    meta: { fetchedAt: 2, status: "partial" as const },
  };
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: SNAPSHOT.meta.fetchedAt });
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: newer.meta.fetchedAt });
  const requests = pendingRequests(h, "get_snapshot");
  assert.equal(requests.length, 2);
  requests[1].reply.resolve(newer);
  await settle();
  assert.deepEqual(h.view.snapshot, newer);
  requests[0].reply.resolve(SNAPSHOT);
  await settle();
  assert.deepEqual(h.view.snapshot, newer);
});

test("workspace switch drops the snapshot and its in-flight fetch", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: SNAPSHOT.meta.fetchedAt });
  h.emit("app-state", { ...IDLE, workspace: "/other", snapshotFetchedAt: null });
  assert.equal(h.view.snapshot, null);
  const request = pendingRequest(h, "get_snapshot");
  request.reply.resolve(SNAPSHOT);
  await settle();
  assert.equal(h.view.snapshot, null);
});

test("an event without snapshotFetchedAt clears a stale snapshot", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, SNAPSHOT);
  assert.deepEqual(h.view.snapshot, SNAPSHOT);
  // Re-opening the same workspace clears the core snapshot without a
  // workspace change, so only this field dropping to null can clear the view.
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: null });
  assert.equal(h.view.snapshot, null);
});

test("loadSnapshot retries a failed fetch directly and deduplicates calls", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: SNAPSHOT.meta.fetchedAt });
  pendingRequest(h, "get_snapshot").reply.reject(new Error("snapshot worker failed"));
  await settle();
  assert.match(h.view.snapshotError ?? "", /snapshot worker failed/);
  void h.session.loadSnapshot();
  void h.session.loadSnapshot();
  assert.equal(h.view.snapshotError, null);
  assert.equal(h.view.snapshotLoading, true);
  assert.equal(requestCount(h, "get_snapshot"), 2);
  lastPendingRequest(h, "get_snapshot").reply.resolve(SNAPSHOT);
  await settle();
  assert.deepEqual(h.view.snapshot, SNAPSHOT);
  assert.equal(h.view.snapshotError, null);
});

test("a failed snapshot fetch is surfaced and retried by the next event", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: SNAPSHOT.meta.fetchedAt });
  pendingRequest(h, "get_snapshot").reply.reject(new Error("snapshot worker failed"));
  await settle();
  assert.match(h.view.snapshotError ?? "", /snapshot worker failed/);
  assert.equal(h.view.snapshotLoading, false);
  h.emit("app-state", {
    ...IDLE,
    snapshotFetchedAt: SNAPSHOT.meta.fetchedAt,
    activeInvocation: "run-a",
  });
  assert.equal(h.view.snapshotError, null);
  lastPendingRequest(h, "get_snapshot").reply.resolve(SNAPSHOT);
  await settle();
  assert.deepEqual(h.view.snapshot, SNAPSHOT);
  assert.equal(h.view.snapshotError, null);
});

test("target details load only after selection and use the current index generation", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  assert.equal(requestCount(h, "get_target_detail"), 0);
  assert.equal(h.view.selection, null);

  h.session.select({ kind: "target", label: "//app:first" });
  assert.deepEqual(pendingRequest(h, "get_target_detail").args, {
    label: "//app:first",
    fetchedAt: TARGETS.meta.fetchedAt,
  });
  assert.equal(h.view.targetDetail, null);
  assert.equal(h.view.targetDetailError, null);
  pendingRequest(h, "get_target_detail").reply.resolve(TARGET_DETAIL);
  await settle();
  assert.deepEqual(h.view.targetDetail, TARGET_DETAIL);
});

test("rapid target clicks keep only the latest result", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "target", label: "//app:first" });
  h.session.select({ kind: "target", label: "//app:second" });
  assert.equal(requestCount(h, "get_target_detail"), 1);
  pendingRequest(h, "get_target_detail").reply.resolve(TARGET_DETAIL);
  await settle();
  const detailAfterStaleReply = h.view.targetDetail;
  assert.equal(detailAfterStaleReply, null);
  assert.deepEqual(lastPendingRequest(h, "get_target_detail").args, {
    label: "//app:second",
    fetchedAt: TARGETS.meta.fetchedAt,
  });
  lastPendingRequest(h, "get_target_detail").reply.resolve({
    ...TARGET_DETAIL,
    target: TARGETS.targets[1],
  });
  await settle();
  assert.equal(h.view.targetDetail?.target.label, "//app:second");
});

test("target detail waits for the Bazel slot and resumes on the idle event", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS, { activeInvocation: RECORD.id });
  h.session.select({ kind: "target", label: "//app:first" });
  assert.equal(requestCount(h, "get_target_detail"), 0);
  assert.equal(h.view.targetDetailError, null);
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: TARGETS.meta.fetchedAt });
  assert.equal(requestCount(h, "get_target_detail"), 1);
  pendingRequest(h, "get_target_detail").reply.resolve(TARGET_DETAIL);
  await settle();
  assert.deepEqual(h.view.targetDetail, TARGET_DETAIL);
});

test("scope and index changes discard an old target detail", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "target", label: "//app:first" });
  const old = pendingRequest(h, "get_target_detail");
  h.emit("app-state", {
    ...IDLE,
    scope: { include: ["//other/..."] },
    snapshotFetchedAt: null,
  });
  old.reply.resolve(TARGET_DETAIL);
  await settle();
  assert.equal(h.view.selection, null);
  assert.equal(h.view.targetDetail, null);

  await emitSnapshot(h, { ...TARGETS, meta: { ...TARGETS.meta, fetchedAt: 2 } });
  assert.equal(requestCount(h, "get_target_detail"), 1);
});

test("a refreshed index replaces an in-flight detail with the new generation", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "target", label: "//app:first" });
  const old = pendingRequest(h, "get_target_detail");

  await emitSnapshot(h, { ...TARGETS, meta: { ...TARGETS.meta, fetchedAt: 2 } });
  const detailAfterRefresh = h.view.targetDetail;
  assert.equal(detailAfterRefresh, null);
  assert.equal(requestCount(h, "get_target_detail"), 1);

  old.reply.resolve(TARGET_DETAIL);
  await settle();
  assert.deepEqual(lastPendingRequest(h, "get_target_detail").args, {
    label: "//app:first",
    fetchedAt: 2,
  });
  const detailAfterStaleReply = h.view.targetDetail;
  assert.equal(detailAfterStaleReply, null);
  lastPendingRequest(h, "get_target_detail").reply.resolve(TARGET_DETAIL);
  await settle();
  assert.equal(h.view.targetDetail?.target.label, "//app:first");
});

test("a target detail failure has an explicit retry", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "target", label: "//app:first" });
  pendingRequest(h, "get_target_detail").reply.reject(new Error("query failed"));
  await settle();
  assert.match(h.view.targetDetailError ?? "", /query failed/);
  h.emit("app-state", { ...IDLE, snapshotFetchedAt: TARGETS.meta.fetchedAt });
  assert.equal(requestCount(h, "get_target_detail"), 1);
  h.session.select({ kind: "target", label: "//app:first" });
  assert.equal(requestCount(h, "get_target_detail"), 2);
});

test("a successful target-list retry resumes the selected detail", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "target", label: "//app:first" });
  pendingRequest(h, "get_target_detail").reply.resolve(TARGET_DETAIL);
  await settle();

  h.emit("app-state", { ...IDLE, snapshotFetchedAt: 2 });
  lastPendingRequest(h, "get_snapshot").reply.reject(new Error("list failed"));
  await settle();
  assert.match(h.view.targetDetailError ?? "", /target list/);
  void h.session.loadSnapshot();
  lastPendingRequest(h, "get_snapshot").reply.resolve({
    ...TARGETS,
    meta: { ...TARGETS.meta, fetchedAt: 2 },
  });
  await settle();
  assert.equal(h.view.targetDetailError, null);
  assert.equal(requestCount(h, "get_target_detail"), 2);
});

test("changing Scope preserves the selected invocation", async () => {
  const h = harness();
  await h.session.init();
  await selectRecord(h);
  h.emit("app-state", { ...IDLE, scope: { include: ["//other/..."] } });
  assert.equal(h.view.selectedInvocation, RECORD.id);
  assert.deepEqual(h.view.invocations, [RECORD]);
});

test("Send passes the pattern through unchanged and ignores whitespace and duplicate starts", async () => {
  const h = harness();
  await h.session.init();
  void h.session.start("build", " //:app ");
  noPending(h, "start_invocation");
  void h.session.start("build", "//:app");
  void h.session.start("build", "//:app");
  assert.equal(requestCount(h, "start_invocation"), 1);
  assert.deepEqual(pendingRequest(h, "start_invocation").args, {
    cmd: "build",
    pattern: "//:app",
    arguments: "",
  });
});

test("start records the selected cmd and the pattern string unchanged", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("test", "//app:tar;get");
  const call = pendingRequest(h, "start_invocation");
  assert.deepEqual(call.args, { cmd: "test", pattern: "//app:tar;get", arguments: "" });
  call.reply.resolve({ ...RECORD, cmd: "test", pattern: "//app:tar;get" });
  await started;
});

test("start forwards quoted arguments and the program separator unchanged", async () => {
  const h = harness();
  await h.session.init();
  const argumentsText = `--config=dev --define='message=hello world' -- "two words" '$HOME; echo'`;
  const started = h.session.start("run", "//:app", argumentsText);
  const call = pendingRequest(h, "start_invocation");
  assert.deepEqual(call.args, {
    cmd: "run",
    pattern: "//:app",
    arguments: argumentsText,
  });
  call.reply.resolve(RECORD);
  await started;
});

test("an argument parse failure is visible and allows a corrected start", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app", '"unfinished');
  pendingRequest(h, "start_invocation").reply.reject(new Error("Unclosed quote in arguments"));
  await started;
  assert.match(h.view.error ?? "", /Unclosed quote/);
  assert.equal(canSend(h.view, "//:app"), true);
  const retried = h.session.start("run", "//:app", '"complete"');
  assert.equal(requestCount(h, "start_invocation"), 2);
  assert.deepEqual(lastPendingRequest(h, "start_invocation").args, {
    cmd: "run",
    pattern: "//:app",
    arguments: '"complete"',
  });
  lastPendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await retried;
  assert.equal(h.view.error, null);
});

test("stop is offered only while an invocation is live and not already stopping", async () => {
  const h = harness();
  assert.equal(canStop(h.view), false);
  await h.session.init();
  assert.equal(canStop(h.view), false);
  await h.session.stop();
  noPending(h, "stop_invocation");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  assert.equal(canStop(h.view), true);
  const stopping = h.session.stop();
  // No optimistic local flag: the core reports stopping through app-state.
  assert.equal(h.view.state?.stopping, false);
  assert.equal(canStop(h.view), true);
  assert.equal(requestCount(h, "stop_invocation"), 1);
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id, stopping: true });
  assert.equal(canStop(h.view), false);
  await h.session.stop();
  assert.equal(requestCount(h, "stop_invocation"), 1);
  pendingRequest(h, "stop_invocation").reply.resolve(undefined);
  await stopping;
});

test("an unobserved invocation keeps composer input in the live Console only", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  assert.equal(h.view.consoleOpen, false);
  pendingRequest(h, "start_invocation").reply.resolve({ id: RECORD.id });
  await started;
  const liveConsole = h.view.console;
  assert.deepEqual(liveConsole, { id: RECORD.id, cmd: "run", pattern: "//:app" });
  assert.equal(h.view.console?.startedAt, undefined);
  pendingRequest(h, "list_invocations").reply.resolve([]);
  await settle();
  h.session.selectInvocation(null);
  assert.equal(h.view.consoleOpen, false);
  const live = h.view.console;
  assert.ok(live);
  h.session.openConsole(live);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(canStop(h.view), true);
});

test("a confirmed build result leaves the live program and Console controls active", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  h.session.openConsole(COMPLETED.record);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(canStop(h.view), true);
  assert.equal(h.view.state?.activeInvocation, RECORD.id);
  noPending(h, "get_invocation");
  const stopping = h.session.stop();
  pendingRequest(h, "stop_invocation").reply.resolve(undefined);
  await stopping;
  assert.equal(h.view.consoleOpen, true);
});

test("stop failure is surfaced and leaves the invocation in flight", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  const stopping = h.session.stop();
  pendingRequest(h, "stop_invocation").reply.reject(new Error("stop unavailable"));
  await stopping;
  assert.match(h.view.error ?? "", /stop unavailable/);
  assert.equal(h.view.state?.activeInvocation, RECORD.id);
});

test("full artifacts are read only after the invocation finishes", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await started;
  assert.equal(h.view.selectedInvocation, null);
  assert.equal(h.view.console?.id, RECORD.id);
  assert.equal(h.view.consoleOpen, true);
  noPending(h, "get_invocation", "get_invocation_profile");
  h.emit("app-state", IDLE);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, RECORD.id);
  lastPendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
  assert.equal(h.view.detail?.log.status, "ok");
});

test("invocation completing before start response opens its detail after reconciliation", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id, consoleInvocation: RECORD.id });
  assert.equal(h.view.consoleOpen, true);
  assert.deepEqual(h.view.console, { id: RECORD.id, cmd: "build", pattern: "//:app" });
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve({ id: RECORD.id });
  await started;
  assert.equal(h.view.state?.activeInvocation, null);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, RECORD.id);
  assert.equal(h.view.console?.id, RECORD.id);
  assert.deepEqual(pendingRequest(h, "get_invocation").args, { id: RECORD.id });
  assert.deepEqual(pendingRequest(h, "get_invocation_profile").args, { id: RECORD.id });
});

test("running state opens Console before the start response and keeps it open", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  assert.equal(h.view.starting, true);
  assert.equal(h.view.consoleOpen, false);
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id, consoleInvocation: RECORD.id });
  assert.equal(h.view.consoleOpen, true);
  const consoleBeforeReply = h.view.console;
  assert.deepEqual(consoleBeforeReply, { id: RECORD.id, cmd: "build", pattern: "//:app" });
  pendingRequest(h, "start_invocation").reply.resolve({ id: RECORD.id });
  await started;
  assert.equal(h.view.starting, false);
  assert.equal(h.view.consoleOpen, true);
  assert.strictEqual(h.view.console, consoleBeforeReply);
});

test("pending start ignores a running state without its Console grant", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id, consoleInvocation: "other-run" });
  assert.equal(h.view.consoleOpen, false);
  pendingRequest(h, "start_invocation").reply.resolve({ id: RECORD.id });
  await started;
});

test("pending start ignores another workspace's running state", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", {
    ...IDLE,
    workspace: "/other",
    activeInvocation: RECORD.id,
    consoleInvocation: RECORD.id,
  });
  assert.equal(h.view.consoleOpen, false);
  pendingRequest(h, "start_invocation").reply.resolve({ id: RECORD.id });
  await started;
  assert.equal(h.view.consoleOpen, false);
});

test("start response before the running event reconciles state without reading active observations", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.setCore({ ...IDLE, activeInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await started;
  assert.equal(h.view.state?.activeInvocation, RECORD.id);
  noPending(h, "get_invocation");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
});

test("completion event during post-start reconciliation wins over its stale running response", async () => {
  const h = harness();
  await h.session.init();
  const reconciled = deferred<AppState>();
  h.nextStateReply(reconciled.promise);
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await settle();
  h.emit("app-state", IDLE);
  reconciled.resolve({ ...IDLE, activeInvocation: RECORD.id });
  await started;
  assert.equal(h.view.state?.activeInvocation, null);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, RECORD.id);
  lastPendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  await settle();
  assert.equal(h.view.detail?.record.exitCode, 0);
});

test("failed state reconciliation waits for a later state event before opening completed detail", async () => {
  const h = harness();
  await h.session.init();
  const reconciled = deferred<AppState>();
  h.nextStateReply(reconciled.promise);
  const started = h.session.start("build", "//:app");
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await settle();
  reconciled.reject(new Error("state unavailable"));
  await started;
  assert.match(h.view.error ?? "", /invocation started/);
  assert.equal(canSend(h.view, "//:app"), false);
  assert.equal(h.view.console?.id, RECORD.id);
  noPending(h, "get_invocation");
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, RECORD.id);
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  await settle();
  assert.equal(h.view.error, null);
  assert.equal(h.view.detail?.record.exitCode, 0);
  assert.equal(canSend(h.view, "//:app"), true);
});

test("state recovery keeps a previous Console reopened while reconciliation was unavailable", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, consoleInvocation: "old-run" });
  const reconciled = deferred<AppState>();
  h.nextStateReply(reconciled.promise);
  const started = h.session.start("build", "//:app");
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await settle();
  reconciled.reject(new Error("state unavailable"));
  await started;
  h.session.openConsole({ id: "old-run" });
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  assert.equal(h.view.ready, true);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(h.view.console?.id, "old-run");
  assert.equal(h.view.selectedInvocation, null);
  noPending(h, "get_invocation");
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

test("a slow previous invocation detail cannot replace the selected invocation", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectInvocation("run-a");
  h.session.selectInvocation("run-b");
  const requests = pendingRequests(h, "get_invocation");
  requests[1].reply.resolve({ ...COMPLETED, record: { ...COMPLETED.record, id: "run-b" } });
  await settle();
  requests[0].reply.resolve(COMPLETED);
  await settle();
  assert.equal(h.view.selectedInvocation, "run-b");
  assert.equal(h.view.detail?.record.id, "run-b");
});

test("workspace switch rejects both stale invocation list and stale detail", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectInvocation("run-a");
  h.emit("app-state", { ...IDLE, workspace: "/other" });
  pendingRequest(h, "list_invocations").reply.resolve([RECORD]);
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.equal(requestCount(h, "list_invocations"), 2);
  assert.deepEqual(h.view.invocations, []);
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
  assert.equal(h.view.profileLoading, false);
  assert.equal(h.view.selectedInvocation, null);
  const otherRecord = { ...RECORD, workspace: "/other" };
  lastPendingRequest(h, "list_invocations").reply.resolve([otherRecord]);
  await settle();
  assert.deepEqual(h.view.invocations, [otherRecord]);
});

test("target selection cancels an in-flight detail without changing core running state", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectInvocation("run-a");
  h.session.selectInvocation(null);
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
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
  pendingRequest(h, "start_invocation").reply.reject(new Error("failed to spawn bazel"));
  await started;
  assert.match(h.view.error ?? "", /failed to spawn/);
  assert.equal(h.view.starting, false);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(canSend(h.view, "//:app"), true);
});

test("failed spawn leaves a previously completed Console open", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  h.session.openConsole(COMPLETED.record);
  const started = h.session.start("build", "//:other");
  assert.equal(h.view.starting, true);
  assert.equal(h.view.console?.id, RECORD.id);
  pendingRequest(h, "start_invocation").reply.reject(new Error("failed to spawn bazel"));
  await started;
  assert.equal(h.view.consoleOpen, true);
  assert.equal(h.view.console?.id, RECORD.id);
  assert.equal(h.view.selectedInvocation, null);
  noPending(h, "get_invocation");
});

test("failed spawn resumes a saved invocation detail selected while start was pending", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.session.selectInvocation("saved-run");
  noPending(h, "get_invocation");
  pendingRequest(h, "start_invocation").reply.reject(new Error("failed to spawn"));
  await started;
  assert.equal(h.view.detailLoading, true);
  const request = pendingRequest(h, "get_invocation");
  assert.deepEqual(request.args, { id: "saved-run" });
  request.reply.resolve({ ...COMPLETED, record: { ...COMPLETED.record, id: "saved-run" } });
  await settle();
  assert.equal(h.view.detail?.record.id, "saved-run");
});

test("dispose removes listeners and ignores late reads", async () => {
  const h = harness();
  await h.session.init();
  h.session.selectInvocation("run-a");
  h.session.dispose();
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.deepEqual(h.removed, ["app-state", "invocations-changed"]);
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
});

test("dispose during start reconciliation ignores completion without reading artifacts", async () => {
  const h = harness();
  await h.session.init();
  const reconciled = deferred<AppState>();
  h.nextStateReply(reconciled.promise);
  const started = h.session.start("build", "//:app");
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await settle();
  const before = h.view;
  h.session.dispose();
  reconciled.resolve({ ...IDLE, consoleInvocation: RECORD.id });
  await started;
  assert.equal(h.view, before);
  assert.deepEqual(h.removed, ["app-state", "invocations-changed"]);
  noPending(h, "get_invocation", "get_invocation_profile");
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

test("failed state subscription removes the history subscription", async () => {
  let view = emptyView();
  const removed: string[] = [];
  const session = createSession(
    {
      listen: async (name) => {
        if (name === "app-state") throw new Error("state listener unavailable");
        return () => removed.push(name);
      },
      invoke: async <T>() => IDLE as T,
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

test("a state event recovers a failed initial state read", async () => {
  const initial = deferred<AppState>();
  const h = harness(initial.promise);
  const initialized = h.session.init();
  await h.stateRequested;
  initial.reject(new Error("state unavailable"));
  await initialized;
  assert.equal(canSend(h.view, "//:app"), false);
  assert.match(h.view.error ?? "", /state unavailable/);
  assert.deepEqual(h.removed, []);
  h.emit("app-state", IDLE);
  assert.equal(canSend(h.view, "//:app"), true);
  assert.equal(h.view.error, null);
  h.session.dispose();
  assert.deepEqual(h.removed, ["app-state", "invocations-changed"]);
});

test("a stale initial read failure cannot disconnect a session established by events", async () => {
  const initial = deferred<AppState>();
  const h = harness(initial.promise);
  const initialized = h.session.init();
  await h.stateRequested;
  h.emit("app-state", IDLE);
  initial.reject(new Error("stale read failure"));
  await initialized;
  assert.equal(h.view.error, null);
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  assert.equal(h.view.state?.activeInvocation, RECORD.id);
  assert.equal(canSend(h.view, "//:app"), false);
  assert.deepEqual(h.removed, []);
});

test("Bazel-busy workspace switch is rejected before calling the backend", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  await assert.rejects(h.session.openWorkspace("/other", IDLE.scope), /Bazel/);
  h.emit("app-state", { ...IDLE, query: "running" });
  await assert.rejects(h.session.openWorkspace("/other", IDLE.scope), /Bazel/);
  noPending(h, "open_workspace");
});

test("refresh is rejected while busy and is never queued", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  await h.session.refresh();
  h.emit("app-state", { ...IDLE, ingest: "running" });
  await h.session.refresh();
  h.emit("app-state", { ...IDLE, query: "running" });
  await h.session.refresh();
  h.emit("app-state", IDLE);
  noPending(h, "refresh");
  const refreshing = h.session.refresh();
  pendingRequest(h, "refresh").reply.resolve(undefined);
  await refreshing;
});

test("an unconfirmed exit stays unknown and is not retried automatically", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id, stopping: true });
  h.session.selectInvocation(RECORD.id);
  h.emit("app-state", { ...IDLE, invocationNote: "Could not confirm the stop" });
  pendingRequest(h, "get_invocation").reply.resolve({ ...COMPLETED, record: RECORD });
  await settle();
  assert.equal(h.view.detail?.record.exitCode, undefined);
  assert.equal(h.view.state?.invocationNote, "Could not confirm the stop");
  noPending(h, "start_invocation", "refresh");
  assert.equal(canSend(h.view, "//:app"), true);
});

test("leaving Console keeps the process and prevents automatic navigation", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await started;
  h.session.selectInvocation(null);
  assert.equal(h.view.state?.activeInvocation, RECORD.id);
  noPending(h, "stop_invocation");
  h.emit("app-state", IDLE);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, null);
});

test("process completion opens detail once and allows Console to stay reopened", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id, consoleInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve({ id: RECORD.id });
  await started;
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, RECORD.id);
  assert.equal(h.view.console?.id, RECORD.id);
  assert.equal(canStop(h.view), false);
  h.session.openConsole({ id: RECORD.id });
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  pendingRequest(h, "list_invocations").reply.resolve([COMPLETED.record]);
  await settle();
  assert.equal(h.view.consoleOpen, true);
  assert.equal(h.view.selectedInvocation, null);
  assert.equal(requestCount(h, "get_invocation"), 1);
  assert.equal(requestCount(h, "get_invocation_profile"), 1);
});

test("successive invocations each open detail once after their own completion", async () => {
  const h = harness();
  await h.session.init();
  const first = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id, consoleInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await first;
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  assert.equal(h.view.selectedInvocation, RECORD.id);
  assert.deepEqual(pendingRequest(h, "get_invocation").args, { id: RECORD.id });
  h.session.openConsole(RECORD);
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  assert.equal(h.view.consoleOpen, true);
  const secondRecord = { ...RECORD, id: "run-b" };
  const second = h.session.start("build", "//:app");
  h.emit("app-state", {
    ...IDLE,
    activeInvocation: secondRecord.id,
    consoleInvocation: secondRecord.id,
  });
  lastPendingRequest(h, "start_invocation").reply.resolve(secondRecord);
  await second;
  h.emit("app-state", { ...IDLE, consoleInvocation: secondRecord.id });
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, secondRecord.id);
  assert.deepEqual(lastPendingRequest(h, "get_invocation").args, { id: secondRecord.id });
  h.session.openConsole(secondRecord);
  h.emit("app-state", { ...IDLE, consoleInvocation: secondRecord.id });
  assert.equal(h.view.consoleOpen, true);
  assert.equal(h.view.selectedInvocation, null);
  assert.equal(requestCount(h, "get_invocation"), 2);
  assert.equal(requestCount(h, "get_invocation_profile"), 2);
});

test("process completion preserves another selected invocation", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id, consoleInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await started;
  h.session.selectInvocation("saved-run");
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, "saved-run");
  assert.deepEqual(pendingRequest(h, "get_invocation").args, { id: "saved-run" });
});

test("completion during start reconciliation preserves a screen selected before the state reply", async () => {
  const h = harness();
  await h.session.init();
  const reconciled = deferred<AppState>();
  h.nextStateReply(reconciled.promise);
  const started = h.session.start("build", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await settle();
  h.session.selectInvocation("saved-run");
  h.emit("app-state", IDLE);
  reconciled.resolve(IDLE);
  await started;
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, "saved-run");
  assert.deepEqual(pendingRequest(h, "get_invocation").args, { id: "saved-run" });
});

test("observed live invocations remain in the workspace list", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  pendingRequest(h, "list_invocations").reply.resolve([RECORD]);
  await settle();
  assert.deepEqual(h.view.invocations, [RECORD]);
});

test("openConsole returns to a Console left while its invocation is in flight", async () => {
  const h = harness();
  await h.session.init();
  const started = h.session.start("run", "//:app");
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  pendingRequest(h, "start_invocation").reply.resolve(RECORD);
  await started;
  h.session.selectInvocation(null);
  assert.equal(h.view.consoleOpen, false);
  h.session.openConsole(RECORD);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(h.view.selectedInvocation, null);
  assert.equal(h.view.console?.id, RECORD.id);
  noPending(h, "stop_invocation", "get_invocation");
});

test("openConsole restores the Console for an invocation already in flight at connect", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  const consoleBeforeOpen = h.view.console;
  assert.equal(consoleBeforeOpen, null);
  h.session.openConsole(RECORD);
  assert.equal(h.view.console?.id, RECORD.id);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(h.view.selectedInvocation, null);
});

test("openConsole cancels an in-flight invocation detail read", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, activeInvocation: RECORD.id });
  h.session.selectInvocation("run-b");
  h.session.openConsole(RECORD);
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
  assert.equal(h.view.profileLoading, false);
  assert.equal(h.view.selectedInvocation, null);
  assert.equal(h.view.consoleOpen, true);
});

test("openConsole on a finished invocation lands on its detail view", async () => {
  const h = harness();
  await h.session.init();
  h.session.openConsole(RECORD);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.selectedInvocation, RECORD.id);
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  await settle();
  assert.equal(h.view.detail?.record.id, RECORD.id);
});

test("a completed Console can be reopened after viewing its build details", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  h.session.openConsole({ id: RECORD.id });
  assert.equal(h.view.consoleOpen, true);
  h.session.selectInvocation(RECORD.id);
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  await settle();
  assert.deepEqual(h.view.detail, COMPLETED);
  h.session.openConsole(COMPLETED.record);
  assert.equal(h.view.consoleOpen, true);
  assert.equal(h.view.selectedInvocation, null);
  assert.equal(canStop(h.view), false);
  assert.equal(h.view.console?.id, RECORD.id);
});

test("workspace changes revoke a completed Console and discard its pending detail response", async () => {
  const h = harness();
  await h.session.init();
  h.emit("app-state", { ...IDLE, consoleInvocation: RECORD.id });
  h.session.openConsole({ id: RECORD.id });
  h.session.selectInvocation(RECORD.id);
  h.emit("app-state", { ...IDLE, workspace: "/other" });
  pendingRequest(h, "get_invocation").reply.resolve(COMPLETED);
  pendingRequest(h, "get_invocation_profile").reply.resolve(PROFILE);
  await settle();
  assert.equal(h.view.console, null);
  assert.equal(h.view.consoleOpen, false);
  assert.equal(h.view.detail, null);
  assert.equal(h.view.profile, null);
  assert.equal(h.view.selectedInvocation, null);
});

test("workspace opening sends all target patterns to the core", async () => {
  const h = harness();
  await h.session.init();
  const scope = {
    include: ["//services/payments/...", "//libs/money/...", "//tools:generator"],
  };
  const opened = h.session.openWorkspace("/other", scope);
  const request = pendingRequest(h, "open_workspace");
  assert.deepEqual(request.args, { path: "/other", scope });
  request.reply.resolve({ ...IDLE, workspace: "/other", scope });
  await opened;
  assert.deepEqual(h.view.state?.scope, scope);
});

test("clearing the target selection rejects an older target detail", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "target", label: "//app:first" });
  h.session.select(null);
  pendingRequest(h, "get_target_detail").reply.resolve(TARGET_DETAIL);
  await settle();
  assert.equal(h.view.selection, null);
  assert.equal(h.view.targetDetail, null);
  assert.equal(requestCount(h, "get_target_detail"), 1);
});

test("package selection makes no request and keeps late target details out", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "target", label: "//app:first" });
  h.session.select({ kind: "package", path: "app" });
  pendingRequest(h, "get_target_detail").reply.resolve(TARGET_DETAIL);
  await settle();
  assert.deepEqual(h.view.selection, { kind: "package", path: "app" });
  assert.equal(h.view.targetDetail, null);
  assert.equal(requestCount(h, "get_target_detail"), 1);
});

test("returning from a package to the same target rejects its earlier reply", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "target", label: "//app:first" });
  h.session.select({ kind: "package", path: "app" });
  h.session.select({ kind: "target", label: "//app:first" });
  assert.equal(requestCount(h, "get_target_detail"), 1);
  pendingRequest(h, "get_target_detail").reply.resolve(TARGET_DETAIL);
  await settle();
  assert.equal(h.view.targetDetail, null);
  assert.equal(requestCount(h, "get_target_detail"), 2);
  lastPendingRequest(h, "get_target_detail").reply.resolve(TARGET_DETAIL);
  await settle();
  assert.deepEqual(h.view.selection, { kind: "target", label: "//app:first" });
  assert.deepEqual(h.view.targetDetail, TARGET_DETAIL);
});

test("snapshot reload and workspace changes close package selection", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "package", path: "app" });
  await emitSnapshot(h, { ...TARGETS, meta: { ...TARGETS.meta, fetchedAt: 2 } });
  assert.equal(h.view.selection, null);
  h.session.select({ kind: "package", path: "app" });
  h.emit("app-state", { ...IDLE, workspace: "/other", snapshotFetchedAt: null });
  assert.equal(h.view.selection, null);
  assert.equal(requestCount(h, "get_target_detail"), 0);
});

test("selecting the same package does not publish another view or query", async () => {
  const h = harness();
  await h.session.init();
  await emitSnapshot(h, TARGETS);
  h.session.select({ kind: "package", path: "app" });
  const selectedView = h.view;
  h.session.select({ kind: "package", path: "app" });
  assert.equal(h.view, selectedView);
  assert.equal(requestCount(h, "get_target_detail"), 0);
});
