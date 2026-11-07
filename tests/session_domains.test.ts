import { test } from "node:test";
import assert from "node:assert/strict";
import { createTargets, emptyTargets } from "../src/lib/session/targets.ts";
import { createInvocations, emptyInvocations } from "../src/lib/session/invocations.ts";
import type { InvocationDetail, InvocationRecord, Section, Profile } from "../src/lib/types.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

async function settle() {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

const SNAPSHOT = {
  meta: { fetchedAt: 1, status: "ok" as const },
  targets: [
    { label: "//app:first", pkg: "app", name: "first", ruleClass: "cc_library", location: null },
  ],
};
const TARGET_DETAIL = {
  target: { ...SNAPSHOT.targets[0], location: { file: "/f", line: 1 } },
  edges: [],
  files: [],
  status: "ok" as const,
};
const RECORD: InvocationRecord = {
  id: "run-a",
  workspace: "/w",
  cmd: "build",
  pattern: "//:a",
  startedAt: 1,
};
const DETAIL: InvocationDetail = {
  record: { ...RECORD, finishedAt: 2, exitCode: 0 },
  log: { status: "ok", data: "" },
  bep: { status: "missing" },
};
const PROFILE: Section<Profile> = { status: "ok", data: { phases: [], lanes: [] } };

test("Targets fetches and selects without any invocation owner", async (t) => {
  const pending: Array<ReturnType<typeof deferred<unknown>>> = [];
  let latest = emptyTargets();
  const targets = createTargets(
    {
      invoke: async <T>() => {
        const r = deferred<unknown>();
        pending.push(r);
        return r.promise as Promise<T>;
      },
    },
    (v) => {
      latest = v;
    },
  );
  t.after(() => targets.dispose());
  targets.sync({ workspace: "/w", scopeKey: "[]", snapshotFetchedAt: 1, canReadDetail: true });
  assert.equal(pending.length, 1);
  pending[0].resolve(SNAPSHOT);
  await settle();
  targets.select({ kind: "target", label: "//app:first" });
  assert.equal(pending.length, 2);
  pending[1].resolve(TARGET_DETAIL);
  await settle();
  assert.deepEqual(latest.snapshot, SNAPSHOT);
  assert.deepEqual(latest.targetDetail, TARGET_DETAIL);
});

test("Invocations loads list and artifacts without any targets owner", async (t) => {
  const pending = new Map<string, ReturnType<typeof deferred<unknown>>>();
  let latest = emptyInvocations();
  const invocations = createInvocations(
    {
      invoke: async <T>(command: string) => {
        const r = deferred<unknown>();
        pending.set(command, r);
        return r.promise as Promise<T>;
      },
    },
    (v) => {
      latest = v;
    },
  );
  t.after(() => invocations.dispose());
  invocations.sync({
    workspace: "/w",
    selectedId: "run-a",
    activeId: null,
    canReadArtifacts: true,
  });
  const list = invocations.loadList();
  pending.get("list_invocations")!.resolve([RECORD]);
  await list;
  assert.deepEqual(latest.records, [RECORD]);
  invocations.loadSelected();
  pending.get("get_invocation")!.resolve(DETAIL);
  pending.get("get_invocation_profile")!.resolve(PROFILE);
  await settle();
  assert.deepEqual(latest.detail, DETAIL);
  assert.deepEqual(latest.profile, PROFILE);
});

test("an in-flight artifact request settles once, then only the latest id is read", async (t) => {
  const requests: Array<{ command: string; id: unknown }> = [];
  const replies = new Map<string, ReturnType<typeof deferred<unknown>>>();
  const invocations = createInvocations(
    {
      invoke: async <T>(command: string, args?: Record<string, unknown>) => {
        requests.push({ command, id: args?.id });
        const r = deferred<unknown>();
        replies.set(`${command}:${args?.id}`, r);
        return r.promise as Promise<T>;
      },
    },
    () => {},
  );
  t.after(() => invocations.dispose());
  invocations.sync({
    workspace: "/w",
    selectedId: "run-a",
    activeId: null,
    canReadArtifacts: true,
  });
  invocations.loadSelected();
  assert.deepEqual(requests, [
    { command: "get_invocation", id: "run-a" },
    { command: "get_invocation_profile", id: "run-a" },
  ]);
  invocations.sync({
    workspace: "/other",
    selectedId: "run-b",
    activeId: null,
    canReadArtifacts: true,
  });
  invocations.loadSelected();
  assert.equal(requests.length, 2);
  replies.get("get_invocation:run-a")!.resolve(DETAIL);
  replies.get("get_invocation_profile:run-a")!.resolve(PROFILE);
  await settle();
  replies
    .get("get_invocation:run-b")!
    .resolve({ ...DETAIL, record: { ...DETAIL.record, id: "run-b" } });
  replies.get("get_invocation_profile:run-b")!.resolve(PROFILE);
  await settle();
  assert.deepEqual(requests, [
    { command: "get_invocation", id: "run-a" },
    { command: "get_invocation_profile", id: "run-a" },
    { command: "get_invocation", id: "run-b" },
    { command: "get_invocation_profile", id: "run-b" },
  ]);
});

test("disposed Targets cannot issue snapshot requests or publish", async (t) => {
  const pending: Array<ReturnType<typeof deferred<unknown>>> = [];
  let publishes = 0;
  const targets = createTargets(
    {
      invoke: async <T>() => {
        const r = deferred<unknown>();
        pending.push(r);
        return r.promise as Promise<T>;
      },
    },
    () => {
      publishes++;
    },
  );
  t.after(() => targets.dispose());
  targets.sync({ workspace: "/w", scopeKey: "[]", snapshotFetchedAt: 1, canReadDetail: true });
  assert.equal(pending.length, 1);
  targets.dispose();
  const publishedAtDispose = publishes;
  targets.sync({ workspace: "/w", scopeKey: "[]", snapshotFetchedAt: 1, canReadDetail: true });
  void targets.loadSnapshot();
  assert.equal(pending.length, 1);
  pending[0].resolve(SNAPSHOT);
  await settle();
  assert.equal(publishes, publishedAtDispose);
});
