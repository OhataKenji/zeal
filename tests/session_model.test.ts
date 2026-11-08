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

  assert.deepEqual(console.content, { kind: "console", page: "targets" });
  assert.deepEqual(selected.model.content, { kind: "targets" });
  assert.equal(selected.model.execution.console?.id, RECORD.id);
  assert.deepEqual(selected.model.targets.selection, { kind: "target", label: "//app:first" });
  const completed = update(selected.model, {
    type: "appState",
    state: { ...active, activeInvocation: null },
  });
  assert.deepEqual(completed.model.content, { kind: "targets" });
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
    assert.deepEqual(selected.model.content, {
      kind: "invocation",
      id: RECORD.id,
      page: "targets",
    });
    assert.deepEqual(finished.model.content, { kind: page });
    assert.equal(finished.model.saved.profile.value, null);
    assert.equal(finished.model.saved.profile.running, null);
    assert.equal(finished.model.saved.profile.wanted, null);
  }
});

test("initial and remembered workspace pages survive workspace changes", () => {
  for (const page of ["targets", "query", "analytics"] as const) {
    const initial = initialModel(page);
    assert.deepEqual(initial.content, { kind: page });
    const connected = update(initial, {
      type: "appState",
      state: { ...state(), activeInvocation: RECORD.id, consoleInvocation: RECORD.id },
    }).model;
    const selected = update(connected, { type: "selectInvocation", id: RECORD.id }).model;
    assert.deepEqual(selected.content, { kind: "invocation", id: RECORD.id, page });
    const console = update(selected, { type: "openConsole", record: RECORD }).model;
    assert.deepEqual(console.content, { kind: "console", page });
    const switched = update(console, {
      type: "appState",
      state: { ...state(), workspace: "/other" },
    });
    assert.deepEqual(switched.model.content, { kind: page });
    assert.equal(switched.model.execution.console, null);
    assert.equal(switched.model.targets.selection, null);
  }
});

test("update is deterministic and leaves nested result and history state untouched", () => {
  const selected = freeze(
    update(indexedModel(), {
      type: "select",
      selection: { kind: "target", label: "//app:first" },
    }).model,
  );
  const detailRequest = selected.targets.detail.running;
  assert.notEqual(detailRequest, null);
  const detail = freeze({
    type: "targetDetailResult" as const,
    requestId: detailRequest!,
    result: { ok: true as const, value: targetDetail("//app:first") },
  });
  const first = update(selected, detail);
  assert.deepEqual(first, update(selected, detail));
  assert.equal(selected.targets.detail.value, null);
  assert.equal(selected.targets.detail.running, detailRequest);
  assert.deepEqual(first.model.targets.detail.value, targetDetail("//app:first"));
  assert.equal(first.model.targets.detail.running, null);
  assert.notEqual(first.model.targets.outputs.wanted, null);

  const history = freeze(update(initialModel(), { type: "appState", state: state() }).model);
  const workspaceChange = freeze({
    type: "appState" as const,
    state: { ...state(), workspace: "/other" },
  });
  const invalidated = update(history, workspaceChange);
  assert.equal(history.history.request?.status, "current");
  assert.equal(invalidated.model.history.request?.status, "invalidated");
  assert.deepEqual(invalidated, update(history, workspaceChange));
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
  assert.equal(model.workspacePrompt.status, "closed");
  assert.equal(opened.model.workspacePrompt.status, "opening");
  assert.deepEqual(opened.effects, [{ ...message, revision: model.connection.revision }]);

  const closing = update(opened.model, { type: "workspacePrompt", open: false });
  assert.equal(closing.model.workspacePrompt.status, "opening");
  const duplicate = update(closing.model, message);
  assert.deepEqual(duplicate.effects, []);
  assert.equal(duplicate.model.workspacePrompt.status, "opening");

  const failed = update(opened.model, {
    type: "workspaceResult",
    revision: model.connection.revision,
    result: { ok: false, error: "open failed" },
  });
  assert.equal(failed.model.workspacePrompt.status, "editing");
  assert.equal(failed.model.workspacePrompt.error, "open failed");
  const retry = update(failed.model, message);
  assert.equal(retry.model.workspacePrompt.error, null);
  assert.equal(retry.model.workspacePrompt.status, "opening");

  const finished = update(retry.model, {
    type: "workspaceResult",
    revision: model.connection.revision,
    result: { ok: true, value: { ...state(null), workspace: "/other" } },
  });
  assert.equal(finished.model.workspacePrompt.status, "closed");
  assert.equal(finished.model.workspacePrompt.error, null);

  const cancelled = update(failed.model, { type: "workspacePrompt", open: false });
  assert.equal(cancelled.model.workspacePrompt.status, "closed");
  assert.equal(cancelled.model.workspacePrompt.error, null);
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
  assert.equal(notified.model.workspacePrompt.status, "opening");
  const failed = update(notified.model, {
    type: "workspaceResult",
    revision: model.connection.revision,
    result: { ok: false, error: "open failed" },
  });
  assert.equal(failed.model.workspacePrompt.status, "editing");
  assert.equal(failed.model.workspacePrompt.error, "open failed");
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
  assert.notEqual(finished.model.saved.detail.running, null);
  assert.equal(finished.model.saved.profile.running, null);
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

test("invalidated coalesced history discards its result and starts one follow-up", () => {
  const initialized = update(initialModel(), { type: "appState", state: state(null) });
  const firstHistory = effect(initialized.effects, "listInvocations");
  const queued = update(initialized.model, { type: "historyChanged" });
  assert.equal(queued.model.history.request?.status, "refresh");

  const changedWorkspace = update(queued.model, {
    type: "appState",
    state: { ...state(null), workspace: "/other" },
  });
  assert.equal(changedWorkspace.model.history.request?.status, "invalidated");
  const notified = update(changedWorkspace.model, { type: "historyChanged" });
  assert.equal(notified.model.history.request?.status, "invalidated");
  assert.deepEqual(notified.effects, []);

  const completed = update(notified.model, {
    type: "invocationsResult",
    requestId: firstHistory.id,
    result: { ok: true, value: [RECORD] },
  });
  assert.deepEqual(completed.model.history.records, []);
  assert.deepEqual(
    completed.effects.map((item) => item.type),
    ["listInvocations"],
  );
  const followUp = effect(completed.effects, "listInvocations");
  assert.notEqual(followUp.id, firstHistory.id);
  assert.deepEqual(completed.model.history.request, { id: followUp.id, status: "current" });
});

test("failed saved refreshes retain cached detail and profile values", () => {
  const initialized = update(initialModel(), { type: "appState", state: state(null) });
  const firstHistory = effect(initialized.effects, "listInvocations");
  const listed = update(initialized.model, {
    type: "invocationsResult",
    requestId: firstHistory.id,
    result: { ok: true, value: [RECORD] },
  });
  const selected = update(listed.model, { type: "selectInvocation", id: RECORD.id });
  const detailRequest = effect(selected.effects, "getInvocation");
  const profileRequest = effect(selected.effects, "getProfile");
  const cachedDetail: InvocationDetail = { ...COMPLETED, record: RECORD };
  const cached = update(
    update(selected.model, {
      type: "detailResult",
      requestId: detailRequest.id,
      result: { ok: true, value: cachedDetail },
    }).model,
    {
      type: "profileResult",
      requestId: profileRequest.id,
      result: { ok: true, value: PROFILE },
    },
  );
  const refresh = update(cached.model, { type: "historyChanged" });
  const refreshRequest = effect(refresh.effects, "listInvocations");
  const newer = update(refresh.model, {
    type: "invocationsResult",
    requestId: refreshRequest.id,
    result: { ok: true, value: [COMPLETED.record] },
  });
  const nextDetail = effect(newer.effects, "getInvocation");
  const nextProfile = effect(newer.effects, "getProfile");
  const detailFailure = update(newer.model, {
    type: "detailResult",
    requestId: nextDetail.id,
    result: { ok: false, error: "detail refresh failed" },
  });
  const profileFailure = update(detailFailure.model, {
    type: "profileResult",
    requestId: nextProfile.id,
    result: { ok: false, error: "profile refresh failed" },
  });

  assert.equal(profileFailure.model.saved.detail.value, cachedDetail);
  assert.equal(profileFailure.model.saved.detail.error, "detail refresh failed");
  assert.equal(profileFailure.model.saved.profile.value, PROFILE);
  assert.equal(profileFailure.model.saved.profile.error, "profile refresh failed");
});
