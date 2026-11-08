import { test } from "node:test";
import assert from "node:assert/strict";
import { initialModel, update, type SessionEffect } from "../src/lib/session_model.ts";
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

function indexedModel() {
  const initialized = update(initialModel(), { type: "appState", state: state() });
  const snapshot = effect(initialized.effects, "getSnapshot");
  return update(initialized.model, {
    type: "snapshotResult",
    requestId: snapshot.id,
    result: { ok: true, value: TARGETS },
  }).model;
}

test("update is deterministic and leaves nested reader state untouched", () => {
  const model = freeze(update(initialModel(), { type: "appState", state: state() }).model);
  const message = freeze({
    type: "select" as const,
    selection: { kind: "target" as const, label: "//app:first" },
  });

  const first = update(model, message);
  const second = update(model, message);

  assert.deepEqual(first, second);
  assert.deepEqual(model.targetDetail, { running: null, wanted: null });
  assert.equal(first.model.targetDetail.running, null);
  assert.notEqual(first.model.targetDetail.wanted, null);
  assert.deepEqual(model.data.state?.scope.include, ["//app/..."]);
});

test("accepted profile result emits no unrelated effects", () => {
  const initialized = update(initialModel(), { type: "appState", state: state(null) });
  const history = effect(initialized.effects, "listInvocations");
  const listed = update(initialized.model, {
    type: "invocationsResult",
    requestId: history.id,
    result: { ok: true, value: [RECORD] },
  });
  const selected = update(listed.model, { type: "selectInvocation", id: RECORD.id });
  const profile = effect(selected.effects, "getProfile");

  const finished = update(selected.model, {
    type: "profileResult",
    requestId: profile.id,
    result: { ok: true, value: PROFILE },
  });

  assert.deepEqual(finished.effects, []);
  assert.notEqual(finished.model.detail.running, null);
  assert.equal(finished.model.profile.running, null);
});

test("accepted target detail emits only its outputs request", () => {
  const indexed = indexedModel();
  const selected = update(indexed, {
    type: "select",
    selection: { kind: "target", label: "//app:first" },
  });
  const detail = effect(selected.effects, "getTargetDetail");

  const finished = update(selected.model, {
    type: "targetDetailResult",
    requestId: detail.id,
    result: { ok: true, value: targetDetail("//app:first") },
  });

  assert.deepEqual(
    finished.effects.map((item) => item.type),
    ["getTargetOutputs"],
  );
  assert.equal(effect(finished.effects, "getTargetOutputs").label, "//app:first");
});

test("stale target completion launches only the latest queued target", () => {
  const indexed = indexedModel();
  const selected = update(indexed, {
    type: "select",
    selection: { kind: "target", label: "//app:first" },
  });
  const first = effect(selected.effects, "getTargetDetail");
  const secondSelection = update(selected.model, {
    type: "select",
    selection: { kind: "target", label: "//app:second" },
  });
  const thirdSelection = update(secondSelection.model, {
    type: "select",
    selection: { kind: "target", label: "//app:first" },
  });

  const stale = update(thirdSelection.model, {
    type: "targetDetailResult",
    requestId: first.id,
    result: { ok: true, value: targetDetail("//app:first") },
  });

  assert.deepEqual(
    stale.effects.map((item) => item.type),
    ["getTargetDetail"],
  );
  assert.equal(effect(stale.effects, "getTargetDetail").label, "//app:first");
  assert.equal(
    stale.effects.some((item) => item.type !== "getTargetDetail"),
    false,
  );
});

test("history reconciliation refreshes saved readers but not targets", () => {
  const initialized = update(initialModel(), { type: "appState", state: state(null) });
  const firstHistory = effect(initialized.effects, "listInvocations");
  const listed = update(initialized.model, {
    type: "invocationsResult",
    requestId: firstHistory.id,
    result: { ok: true, value: [COMPLETED.record] },
  });
  const selected = update(listed.model, { type: "selectInvocation", id: RECORD.id });
  const detailRequest = effect(selected.effects, "getInvocation");
  const profileRequest = effect(selected.effects, "getProfile");
  const staleDetail: InvocationDetail = { ...COMPLETED, record: RECORD };
  const detail = update(selected.model, {
    type: "detailResult",
    requestId: detailRequest.id,
    result: { ok: true, value: staleDetail },
  });
  const profile = update(detail.model, {
    type: "profileResult",
    requestId: profileRequest.id,
    result: { ok: true, value: PROFILE },
  });
  const refresh = update(profile.model, { type: "historyChanged" });
  const refreshRequest = effect(refresh.effects, "listInvocations");
  const queued = update(refresh.model, { type: "historyChanged" });
  const queuedAgain = update(queued.model, { type: "historyChanged" });

  const reconciled = update(queuedAgain.model, {
    type: "invocationsResult",
    requestId: refreshRequest.id,
    result: { ok: true, value: [COMPLETED.record] },
  });

  assert.deepEqual(reconciled.effects.map((item) => item.type).sort(), [
    "getInvocation",
    "getProfile",
    "listInvocations",
  ]);
});
