import { test } from "node:test";
import assert from "node:assert/strict";
import { initialModel, sessionView, update, type SessionEffect } from "../src/lib/session_model.ts";
import type {
  AppState,
  InvocationDetail,
  InvocationRecord,
  Profile,
  Section,
  TargetDetail,
  TargetIndex,
} from "../src/lib/types.ts";

function state(snapshotFetchedAt: number | null = 1): AppState {
  return {
    besReady: false,
    scope: { include: ["//app/..."] },
    workspace: "/workspace",
    bazel: "/bin/bazel",
    ingest: "idle",
    snapshotFetchedAt,
    lastError: null,
    activeInvocation: null,
    stopping: false,
    invocationNote: null,
    detailQuery: "idle",
  };
}

const RECORD: InvocationRecord = {
  id: "run-a",
  workspace: "/workspace",
  cmd: "build",
  pattern: "//:app",
  startedAt: 1,
};
const COMPLETED: InvocationDetail = {
  record: { ...RECORD, finishedAt: 2, exitCode: 0 },
  log: { status: "ok", data: "done\n" },
  bep: { status: "missing" },
};
const PROFILE: Section<Profile> = { status: "missing" };
const TARGETS: TargetIndex = {
  meta: { fetchedAt: 1, status: "ok" },
  targets: ["//app:first", "//app:second"].map((label) => ({
    label,
    pkg: "app",
    name: label.split(":")[1],
    ruleClass: "cc_library",
    location: null,
  })),
};

function targetDetail(label: string): TargetDetail {
  const target = TARGETS.targets.find((item) => item.label === label);
  assert.ok(target);
  return { target, edges: [], files: [], status: "ok" };
}

function effect<T extends SessionEffect["type"]>(
  effects: SessionEffect[],
  type: T,
): Extract<SessionEffect, { type: T }> {
  const found = effects.find(
    (item): item is Extract<SessionEffect, { type: T }> => item.type === type,
  );
  assert.ok(found, `Expected a ${type} effect`);
  return found;
}

function freeze<T>(value: T): T {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.values(value).forEach((child) => freeze(child));
    Object.freeze(value);
  }
  return value;
}

test("update is deterministic and leaves nested request state untouched", () => {
  const initialized = update(initialModel(), { type: "appState", state: state() });
  const model = freeze(initialized.model);
  const message = freeze({ type: "historyChanged" as const });

  const first = update(model, message);
  const second = update(model, message);

  assert.deepEqual(first, second);
  assert.equal(model.historyAgain, false);
  assert.equal(first.model.historyAgain, true);
  assert.equal(model.historyRequest?.workspace, "/workspace");
  assert.deepEqual(model.data.state?.scope.include, ["//app/..."]);
});

test("a delayed target detail only starts outputs for the latest selection", () => {
  const initial = update(initialModel(), { type: "appState", state: state() });
  const snapshotRequest = effect(initial.effects, "getSnapshot");
  const firstSelection = update(initial.model, {
    type: "select",
    selection: { kind: "target", label: "//app:first" },
  });
  const snapshot = update(firstSelection.model, {
    type: "snapshotLoaded",
    requestId: snapshotRequest.requestId,
    snapshot: TARGETS,
  });
  const firstDetail = effect(snapshot.effects, "getTargetDetail");
  const secondSelection = update(snapshot.model, {
    type: "select",
    selection: { kind: "target", label: "//app:second" },
  });

  assert.equal(secondSelection.effects.length, 0);
  const stale = update(secondSelection.model, {
    type: "targetDetailLoaded",
    requestId: firstDetail.requestId,
    detail: targetDetail("//app:first"),
  });
  const secondDetail = effect(stale.effects, "getTargetDetail");
  assert.equal(secondDetail.request.label, "//app:second");
  assert.equal(
    stale.effects.some((item) => item.type === "getTargetOutputs"),
    false,
  );

  const loaded = update(stale.model, {
    type: "targetDetailLoaded",
    requestId: secondDetail.requestId,
    detail: targetDetail("//app:second"),
  });
  const outputs = effect(loaded.effects, "getTargetOutputs");
  assert.equal(outputs.request.label, "//app:second");
  const finished = update(loaded.model, {
    type: "targetOutputsLoaded",
    requestId: outputs.requestId,
    outputs: { paths: ["app/second.a"], truncated: false },
  });
  assert.deepEqual(sessionView(finished.model).targetOutputs?.paths, ["app/second.a"]);
});

test("same-history reconciliation refreshes stale saved data and coalesces notifications", () => {
  const firstState = update(initialModel(), { type: "appState", state: state(null) });
  const firstHistory = effect(firstState.effects, "listInvocations");
  const queuedOne = update(firstState.model, { type: "historyChanged" });
  const queuedTwo = update(queuedOne.model, { type: "historyChanged" });
  const historyAfterQueue = update(queuedTwo.model, {
    type: "invocationsLoaded",
    requestId: firstHistory.requestId,
    records: [COMPLETED.record],
  });
  assert.equal(
    historyAfterQueue.effects.filter((item) => item.type === "listInvocations").length,
    1,
  );

  const followup = effect(historyAfterQueue.effects, "listInvocations");
  const listed = update(historyAfterQueue.model, {
    type: "invocationsLoaded",
    requestId: followup.requestId,
    records: [COMPLETED.record],
  });
  const selected = update(listed.model, { type: "selectInvocation", id: RECORD.id });
  const detailRequest = effect(selected.effects, "getInvocation");
  const profileRequest = effect(selected.effects, "getProfile");
  const staleDetail: InvocationDetail = { ...COMPLETED, record: RECORD };
  const loadedOld = update(selected.model, {
    type: "detailLoaded",
    requestId: detailRequest.requestId,
    detail: staleDetail,
  });
  const loadedProfile = update(loadedOld.model, {
    type: "profileLoaded",
    requestId: profileRequest.requestId,
    profile: PROFILE,
  });

  const refreshList = update(loadedProfile.model, { type: "historyChanged" });
  const listRequest = effect(refreshList.effects, "listInvocations");
  const refreshed = update(refreshList.model, {
    type: "invocationsLoaded",
    requestId: listRequest.requestId,
    records: [COMPLETED.record],
  });
  const detailRefresh = effect(refreshed.effects, "getInvocation");
  const profileRefresh = effect(refreshed.effects, "getProfile");

  const staleSuccess = update(refreshed.model, {
    type: "detailLoaded",
    requestId: detailRefresh.requestId,
    detail: staleDetail,
  });
  const missingProfile = update(staleSuccess.model, {
    type: "profileLoaded",
    requestId: profileRefresh.requestId,
    profile: PROFILE,
  });
  assert.equal(missingProfile.effects.length, 0);

  const nextHistory = update(missingProfile.model, { type: "historyChanged" });
  const nextListRequest = effect(nextHistory.effects, "listInvocations");
  const nextRefresh = update(nextHistory.model, {
    type: "invocationsLoaded",
    requestId: nextListRequest.requestId,
    records: [COMPLETED.record],
  });
  assert.equal(nextRefresh.effects.filter((item) => item.type === "getInvocation").length, 1);
  assert.equal(nextRefresh.effects.filter((item) => item.type === "getProfile").length, 1);
});
