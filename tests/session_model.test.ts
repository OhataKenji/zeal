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

test("selecting a target leaves Console in one transition and survives completion", () => {
  const active = { ...state(), activeInvocation: RECORD.id, consoleInvocation: RECORD.id };
  const observed = update(indexedModel(), { type: "appState", state: active }).model;
  const console = freeze(update(observed, { type: "openConsole", record: RECORD }).model);
  const selected = update(console, {
    type: "select",
    selection: { kind: "target", label: "//app:first" },
  });

  assert.deepEqual(console.data.content, { kind: "console" });
  assert.deepEqual(selected.model.data.content, { kind: "targets" });
  assert.equal(selected.model.data.console?.id, RECORD.id);
  assert.deepEqual(selected.model.data.selection, { kind: "target", label: "//app:first" });
  const completed = update(selected.model, {
    type: "appState",
    state: { ...active, activeInvocation: null },
  });
  assert.deepEqual(completed.model.data.content, { kind: "targets" });
  assert.equal(effect(completed.effects, "getTargetDetail").label, "//app:first");
});

test("workspace pages own navigation and reject a previous invocation's profile", () => {
  for (const page of ["query", "analytics"] as const) {
    const selected = update(indexedModel(), { type: "selectInvocation", id: RECORD.id });
    const profile = effect(selected.effects, "getProfile");
    const navigated = update(freeze(selected.model), { type: "showPage", page });
    const finished = update(navigated.model, {
      type: "profileResult",
      requestId: profile.id,
      result: { ok: true, value: PROFILE },
    });
    assert.deepEqual(selected.model.data.content, { kind: "invocation", id: RECORD.id });
    assert.deepEqual(finished.model.data.content, { kind: page });
    assert.equal(finished.model.data.profile, null);
    assert.equal(finished.model.profile.running, null);
    assert.equal(finished.model.profile.wanted, null);
  }
});

test("initial and remembered workspace pages survive workspace changes", () => {
  for (const page of ["targets", "query", "analytics"] as const) {
    const initial = initialModel(page);
    assert.deepEqual(initial.data.content, { kind: page });
    const connected = update(initial, { type: "appState", state: state() }).model;
    const selected = update(connected, { type: "selectInvocation", id: RECORD.id }).model;
    const switched = update(selected, {
      type: "appState",
      state: { ...state(), workspace: "/other" },
    });
    assert.deepEqual(switched.model.data.content, { kind: page });
    assert.equal(switched.model.data.console, null);
    assert.equal(switched.model.data.selection, null);
  }
});

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

test("workspace opening is a pure transition and controls its prompt through results", () => {
  const model = freeze(update(initialModel(), { type: "appState", state: state(null) }).model);
  const message = freeze({
    type: "openWorkspace" as const,
    path: "/other",
    scope: { include: ["//..."] },
  });
  const opened = update(model, message);
  assert.deepEqual(update(model, message), opened);
  assert.equal(model.opening, false);
  assert.equal(model.data.workspacePromptOpen, false);
  assert.equal(opened.model.opening, true);
  assert.equal(opened.model.data.workspacePromptOpen, true);
  assert.deepEqual(opened.effects, [{ ...message, revision: model.stateRevision }]);

  const closing = update(opened.model, { type: "workspacePrompt", open: false });
  assert.equal(closing.model.data.workspacePromptOpen, true);
  const duplicate = update(closing.model, message);
  assert.deepEqual(duplicate.effects, []);
  assert.equal(duplicate.model.opening, true);

  const failed = update(opened.model, {
    type: "workspaceResult",
    revision: model.stateRevision,
    result: { ok: false, error: "open failed" },
  });
  assert.equal(failed.model.opening, false);
  assert.equal(failed.model.data.workspacePromptOpen, true);
  assert.equal(failed.model.data.workspaceError, "open failed");
  const retry = update(failed.model, message);
  assert.equal(retry.model.data.workspaceError, null);
  assert.equal(retry.model.opening, true);

  const finished = update(retry.model, {
    type: "workspaceResult",
    revision: model.stateRevision,
    result: { ok: true, value: { ...state(null), workspace: "/other" } },
  });
  assert.equal(finished.model.opening, false);
  assert.equal(finished.model.data.workspacePromptOpen, false);
  assert.equal(finished.model.data.workspaceError, null);

  const cancelled = update(failed.model, { type: "workspacePrompt", open: false });
  assert.equal(cancelled.model.data.workspacePromptOpen, false);
  assert.equal(cancelled.model.data.workspaceError, null);
  assert.deepEqual(cancelled.effects, []);
});

test("workspace state notifications do not close a pending initial prompt", () => {
  const model = update(initialModel(), {
    type: "appState",
    state: { ...state(null), workspace: null },
  }).model;
  const opening = update(model, {
    type: "openWorkspace",
    path: "/workspace",
    scope: state().scope,
  });
  const notified = update(opening.model, { type: "appState", state: state(null) });
  assert.equal(notified.model.opening, true);
  assert.equal(notified.model.data.workspacePromptOpen, true);
  const failed = update(notified.model, {
    type: "workspaceResult",
    revision: model.stateRevision,
    result: { ok: false, error: "open failed" },
  });
  assert.equal(failed.model.opening, false);
  assert.equal(failed.model.data.workspacePromptOpen, true);
  assert.equal(failed.model.data.workspaceError, "open failed");
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
