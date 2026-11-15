import { test } from "node:test";
import assert from "node:assert/strict";
import {
  initialModel,
  sessionView,
  update,
  type SessionEffect,
  type SessionModel,
} from "../src/lib/session_model.ts";
import type {
  AppState,
  InvocationDetail,
  InvocationRecord,
  OwnedInvocation,
  TargetDetail,
  TargetIndex,
  WorkspaceState,
} from "../src/lib/types.ts";

const WS = "/workspace";
const OTHER = "/other";

function wsEntry(workspace: string, over: Partial<WorkspaceState> = {}): WorkspaceState {
  return {
    workspace,
    scope: { include: ["//app/..."] },
    ingest: "idle",
    snapshotFetchedAt: 1,
    lastError: null,
    ...over,
  };
}

function owned(
  id: string,
  workspace: string,
  over: Partial<OwnedInvocation> = {},
): OwnedInvocation {
  return {
    id,
    workspace,
    cmd: "build",
    pattern: "//:app",
    startedAt: 1,
    running: true,
    stoppable: true,
    stopping: false,
    console: true,
    processExitCode: null,
    note: null,
    ...over,
  };
}

function state(
  over: Partial<AppState> = {},
  workspaces: WorkspaceState[] = [wsEntry(WS)],
  invocations: OwnedInvocation[] = [],
): AppState {
  return {
    revision: 1,
    besError: null,
    besReady: true,
    bazel: "/bin/bazel",
    lastWorkspace: WS,
    workspaces,
    invocations,
    ...over,
  };
}

const RECORD: InvocationRecord = {
  id: "run-a",
  workspace: WS,
  cmd: "build",
  pattern: "//:app",
  startedAt: 1,
};
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

function connectedModel(
  core: AppState = state(),
  page: "targets" | "analytics" | "query" = "targets",
) {
  return update(initialModel(page), { type: "appState", state: core }).model;
}

// The history reader coalesces: a workspace switch marks the in-flight read
// invalidated and only issues the new workspace's read once it lands.
function settleHistory(model: SessionModel): SessionModel {
  const request = model.history.request;
  if (!request) return model;
  return update(model, {
    type: "invocationsResult",
    requestId: request.id,
    result: { ok: true, value: [] },
  }).model;
}

function indexedModel(core: AppState = state()) {
  const initialized = update(initialModel(), { type: "appState", state: core });
  const snapshot = effect(initialized.effects, "getSnapshot");
  return update(initialized.model, {
    type: "snapshotResult",
    requestId: snapshot.id,
    result: { ok: true, value: TARGETS },
  }).model;
}

test("stale and equal revisions are ignored, including get_state replies", () => {
  const first = connectedModel(state({ revision: 5 }));
  const stale = update(first, {
    type: "appState",
    state: state({ revision: 4, besError: "old" }),
  });
  assert.equal(stale.model.connection.state?.revision, 5);
  assert.equal(stale.model.connection.state?.besError, null);
  const equal = update(first, {
    type: "appState",
    state: state({ revision: 5, besError: "same" }),
  });
  assert.equal(equal.model.connection.state?.besError, null);
  const reply = update(first, {
    type: "stateResult",
    result: { ok: true, value: state({ revision: 3 }) },
  });
  assert.equal(reply.model.connection.state?.revision, 5);
  const newer = update(first, {
    type: "stateResult",
    result: { ok: true, value: state({ revision: 6, besError: "new" }) },
  });
  assert.equal(newer.model.connection.state?.revision, 6);
  assert.equal(newer.model.connection.state?.besError, "new");
});

test("initial restore selects lastWorkspace once and never again", () => {
  const initial = connectedModel(state({ lastWorkspace: OTHER }, [wsEntry(WS), wsEntry(OTHER)]));
  assert.equal(initial.workspace, OTHER);
  const later = update(initial, {
    type: "appState",
    state: state({ revision: 2, lastWorkspace: WS }, [wsEntry(WS), wsEntry(OTHER)]),
  });
  assert.equal(later.model.workspace, OTHER);
});

test("history, snapshot, and refresh effects carry the selected workspace", () => {
  const two = state({}, [wsEntry(WS), wsEntry(OTHER)]);
  const initial = update(initialModel(), { type: "appState", state: two });
  assert.equal(initial.model.workspace, WS);
  assert.equal(effect(initial.effects, "listInvocations").workspace, WS);
  assert.equal(effect(initial.effects, "getSnapshot").workspace, WS);
  const settled = settleHistory(initial.model);
  const opened = update(settled, {
    type: "openWorkspace",
    path: OTHER,
    scope: { include: ["//..."] },
  });
  const reply = update(opened.model, {
    type: "workspaceResult",
    requestId: opened.model.workspacePrompt.requestId!,
    result: {
      ok: true,
      value: {
        workspace: OTHER,
        state: state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER)]),
      },
    },
  });
  assert.equal(reply.model.workspace, OTHER);
  for (const item of reply.effects) {
    if (item.type === "listInvocations" || item.type === "getSnapshot") {
      assert.equal(item.workspace, OTHER);
    }
  }
  assert.ok(reply.effects.some((item) => item.type === "listInvocations"));
  assert.ok(reply.effects.some((item) => item.type === "getSnapshot"));
});

test("openWorkspace reply selects its canonical workspace even with a stale snapshot", () => {
  const current = state({ revision: 9 }, [wsEntry(WS), wsEntry(OTHER)]);
  const model = settleHistory(connectedModel(current));
  const opened = update(model, {
    type: "openWorkspace",
    path: "/alias/of/other",
    scope: { include: ["//..."] },
  });
  const requestId = opened.model.workspacePrompt.requestId;
  assert.ok(requestId);
  const reply = update(opened.model, {
    type: "workspaceResult",
    requestId,
    result: {
      ok: true,
      value: {
        workspace: OTHER,
        state: state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER)]),
      },
    },
  });
  // The embedded state is older than revision 9, but the selection still lands.
  assert.equal(reply.model.connection.state?.revision, 9);
  assert.equal(reply.model.workspace, OTHER);
  assert.equal(reply.model.workspacePrompt.status, "closed");
  // OTHER has a snapshot, so no refresh is scheduled; history + snapshot load.
  assert.ok(reply.effects.some((item) => item.type === "listInvocations"));
  assert.ok(reply.effects.some((item) => item.type === "getSnapshot"));
  assert.equal(
    reply.effects.some((item) => item.type === "refresh"),
    false,
  );
});

test("openWorkspace without a snapshot schedules exactly one refresh", () => {
  const model = connectedModel(state());
  const opened = update(model, {
    type: "openWorkspace",
    path: OTHER,
    scope: { include: ["//..."] },
  });
  const requestId = opened.model.workspacePrompt.requestId;
  const bare = wsEntry(OTHER, { snapshotFetchedAt: null });
  const reply = update(opened.model, {
    type: "workspaceResult",
    requestId: requestId!,
    result: {
      ok: true,
      value: {
        workspace: OTHER,
        state: state({ revision: 2 }, [wsEntry(WS), bare]),
      },
    },
  });
  // Embedded revision 2 < current 1? current is 1, so it applies and OTHER has no snapshot.
  assert.equal(reply.model.workspace, OTHER);
  assert.equal(reply.effects.filter((item) => item.type === "refresh").length, 1);
  const again = update(reply.model, {
    type: "appState",
    state: state({ revision: 3 }, [wsEntry(WS), bare]),
  });
  assert.equal(again.effects.filter((item) => item.type === "refresh").length, 0);
});

test("a stale openWorkspace reply cannot steal the selection", () => {
  const model = connectedModel(state());
  const first = update(model, {
    type: "openWorkspace",
    path: OTHER,
    scope: { include: ["//a"] },
  });
  const second = update(first.model, {
    type: "openWorkspace",
    path: "/third",
    scope: { include: ["//b"] },
  });
  const staleReply = update(second.model, {
    type: "workspaceResult",
    requestId: first.model.workspacePrompt.requestId!,
    result: {
      ok: true,
      value: { workspace: OTHER, state: state({ revision: 2 }) },
    },
  });
  assert.equal(staleReply.model.workspace, WS);
  const freshReply = update(staleReply.model, {
    type: "workspaceResult",
    requestId: second.model.workspacePrompt.requestId!,
    result: {
      ok: true,
      value: { workspace: "/third", state: state({ revision: 3 }) },
    },
  });
  assert.equal(freshReply.model.workspace, "/third");
});

test("failed openWorkspace keeps the current selection", () => {
  const model = connectedModel(state());
  const opened = update(model, {
    type: "openWorkspace",
    path: OTHER,
    scope: { include: ["//..."] },
  });
  const failed = update(opened.model, {
    type: "workspaceResult",
    requestId: opened.model.workspacePrompt.requestId!,
    result: { ok: false, error: "no such directory" },
  });
  assert.equal(failed.model.workspace, WS);
  assert.equal(failed.model.workspacePrompt.status, "editing");
  assert.equal(failed.model.workspacePrompt.error, "no such directory");
});

test("start captures workspace and navigates only while it stays current", () => {
  const model = indexedModel(state({}, [wsEntry(WS), wsEntry(OTHER)]));
  const started = update(model, {
    type: "start",
    cmd: "build",
    pattern: "//app:first",
    argumentsText: "",
  });
  const request = effect(started.effects, "startInvocation");
  assert.equal(request.workspace, WS);
  const execution = owned("inv-1", WS);
  const replied = update(started.model, {
    type: "startResult",
    requestId: request.id,
    result: { ok: true, value: execution },
  });
  assert.deepEqual(replied.model.content, { kind: "console", id: "inv-1", page: "targets" });
  assert.deepEqual(replied.effects, []);
  assert.equal(replied.model.starts[request.id], undefined);
});

test("a delayed start reply cannot navigate or error after a workspace switch", () => {
  const model = indexedModel(state({}, [wsEntry(WS), wsEntry(OTHER)]));
  const started = update(model, {
    type: "start",
    cmd: "build",
    pattern: "//app:first",
    argumentsText: "",
  });
  const request = effect(started.effects, "startInvocation");
  // Explicitly open OTHER before the A reply lands.
  const opened = update(started.model, {
    type: "openWorkspace",
    path: OTHER,
    scope: { include: ["//..."] },
  });
  const switched = update(opened.model, {
    type: "workspaceResult",
    requestId: opened.model.workspacePrompt.requestId!,
    result: {
      ok: true,
      value: {
        workspace: OTHER,
        state: state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER)]),
      },
    },
  });
  const failed = update(switched.model, {
    type: "startResult",
    requestId: request.id,
    result: { ok: false, error: "bazel missing" },
  });
  assert.equal(failed.model.workspace, OTHER);
  assert.equal(failed.model.connection.error, null);
  assert.notEqual(failed.model.content.kind, "console");
});

test("manual navigation after start blocks the console jump", () => {
  const model = indexedModel();
  const started = update(model, {
    type: "start",
    cmd: "run",
    pattern: "//app:first",
    argumentsText: "",
  });
  const request = effect(started.effects, "startInvocation");
  const navigated = update(started.model, { type: "showPage", page: "query" });
  const replied = update(navigated.model, {
    type: "startResult",
    requestId: request.id,
    result: { ok: true, value: owned("inv-9", WS, { cmd: "run" }) },
  });
  assert.deepEqual(replied.model.content, { kind: "query" });
  assert.deepEqual(replied.effects, []);
});

test("stop targets the owned execution's own workspace and id", () => {
  const foreign = owned("b-run", OTHER);
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER)], [foreign]));
  const stopped = update(model, { type: "stop", id: "b-run" });
  const request = effect(stopped.effects, "stop");
  assert.equal(request.workspace, OTHER);
  assert.equal(request.id, "b-run");
});

test("stop rejects finished, stopping, and external invocations", () => {
  const finished = owned("done", WS, { running: false, stoppable: false });
  const stopping = owned("halting", WS, { stopping: true });
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS)], [finished, stopping]));
  for (const id of ["done", "halting", "external-bep-only"]) {
    const next = update(model, { type: "stop", id });
    assert.equal(next.effects.length, 0, id);
  }
});

test("stop stays available for a run whose BEP record already succeeded", () => {
  const running = owned("run-1", WS, { cmd: "run" });
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS)], [running]));
  // BEP history may already contain exit 0 while the process still runs.
  const withRecord: typeof model = {
    ...model,
    history: {
      ...model.history,
      records: [{ ...RECORD, id: "run-1", exitCode: 0, finishedAt: 2 }],
    },
  };
  assert.equal(
    sessionView(withRecord).executions.find((entry) => entry.id === "run-1")?.stoppable,
    true,
  );
  const stopped = update(withRecord, { type: "stop", id: "run-1" });
  assert.equal(effect(stopped.effects, "stop").workspace, WS);
});

test("executions view lists running and console-owned entries for the selected workspace", () => {
  const a = owned("a-run", WS);
  const finished = owned("a-done", WS, {
    running: false,
    stoppable: false,
    processExitCode: 0,
  });
  const bepBacked = owned("a-bep", WS, { running: false, stoppable: false });
  const query = owned("a-query", WS, { cmd: "query", console: false });
  const other = owned("b-run", OTHER);
  const model = connectedModel(
    state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER)], [a, finished, bepBacked, query, other]),
  );
  const withHistory = {
    ...model,
    history: { ...model.history, records: [{ ...RECORD, id: "a-bep" }] },
  };
  const view = sessionView(withHistory);
  assert.deepEqual(
    view.executions.map((entry) => entry.id).sort(),
    ["a-done", "a-query", "a-run"].sort(),
  );
});

test("console content carries its invocation id and derives the display record", () => {
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS)], [owned("inv-1", WS)]));
  const opened = update(model, { type: "openConsole", id: "inv-1" });
  assert.deepEqual(opened.model.content, { kind: "console", id: "inv-1", page: "targets" });
  const view = sessionView(opened.model);
  assert.equal(view.console?.id, "inv-1");
  assert.equal(view.console?.workspace, WS);
  assert.equal(view.execution?.id, "inv-1");
});

test("openConsole falls back to the invocation page without a console", () => {
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS)], []));
  const opened = update(model, { type: "openConsole", id: "bep-only" });
  assert.deepEqual(opened.model.content, {
    kind: "invocation",
    id: "bep-only",
    page: "targets",
  });
});

test("saved reads require the id in BEP history, not just an owned entry", () => {
  const ownedOnly = owned("query-1", WS, { cmd: "query", console: false });
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS)], [ownedOnly]));
  const selected = update(model, { type: "selectInvocation", id: "query-1" });
  assert.equal(
    selected.effects.some((item) => item.type === "getInvocation" || item.type === "getProfile"),
    false,
  );
});

test("target detail carries the workspace and drops stale selections on scope change", () => {
  const model = indexedModel();
  const selected = update(model, {
    type: "select",
    selection: { kind: "target", label: "//app:first" },
  });
  const detail = effect(selected.effects, "getTargetDetail");
  assert.equal(detail.workspace, WS);
  const rescoped = update(selected.model, {
    type: "appState",
    state: state({ revision: 2 }, [wsEntry(WS, { scope: { include: ["//other/..."] } })]),
  });
  assert.equal(rescoped.model.targets.selection, null);
  assert.equal(rescoped.model.targets.detail.value, null);
});

test("refresh uses the selected workspace", () => {
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER)]));
  const refreshed = update(model, { type: "refresh" });
  assert.equal(effect(refreshed.effects, "refresh").workspace, WS);
});

test("stopResult surfaces errors only while the page targets that invocation", () => {
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS)], [owned("a", WS)]));
  const selected = update(model, { type: "selectInvocation", id: "a" });
  const failed = update(selected.model, {
    type: "stopResult",
    id: "a",
    result: { ok: false, error: "stop failed" },
  });
  assert.equal(failed.model.connection.error, "stop failed");
  const elsewhere = update(model, { type: "showPage", page: "targets" });
  const hidden = update(elsewhere.model, {
    type: "stopResult",
    id: "a",
    result: { ok: false, error: "stop failed" },
  });
  assert.equal(hidden.model.connection.error, null);
});

test("update is deterministic and leaves nested state untouched", () => {
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
  assert.deepEqual(first.model.targets.detail.value, targetDetail("//app:first"));
});

const OTHER_RECORD: InvocationRecord = { ...RECORD, id: "b-id", workspace: OTHER };
const A_DETAIL: InvocationDetail = {
  record: { ...RECORD, id: "a-id", finishedAt: 2, exitCode: 0 },
  log: { status: "missing" },
  bep: { status: "missing" },
};
const B_DETAIL: InvocationDetail = {
  record: { ...OTHER_RECORD, finishedAt: 3, exitCode: 0 },
  log: { status: "missing" },
  bep: { status: "missing" },
};

function openReply(
  model: SessionModel,
  path: string,
  scope: { include: string[] },
  core: AppState,
) {
  const opened = update(model, { type: "openWorkspace", path, scope });
  return update(opened.model, {
    type: "workspaceResult",
    requestId: opened.model.workspacePrompt.requestId!,
    result: { ok: true, value: { workspace: path, state: core } },
  });
}

test("a workspace switch keeps occupied saved readers until their results settle", () => {
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER)]));
  const withA: typeof model = {
    ...model,
    history: { ...model.history, records: [{ ...RECORD, id: "a-id" }] },
  };
  const selectedA = update(withA, { type: "selectInvocation", id: "a-id" });
  const detailReq = effect(selectedA.effects, "getInvocation");
  const profileReq = effect(selectedA.effects, "getProfile");
  const switched = openReply(
    selectedA.model,
    OTHER,
    { include: ["//..."] },
    state({ revision: 3 }, [wsEntry(WS), wsEntry(OTHER)]),
  );
  assert.equal(switched.model.workspace, OTHER);
  // B's history arrives and its record is selected while A's workers run.
  const withB: typeof switched.model = {
    ...switched.model,
    history: { ...switched.model.history, records: [OTHER_RECORD] },
  };
  const selectedB = update(withB, { type: "selectInvocation", id: "b-id" });
  assert.equal(
    selectedB.effects.some((item) => item.type === "getInvocation" || item.type === "getProfile"),
    false,
  );
  // A's late replies release the workers but cannot populate B.
  const detailDone = update(selectedB.model, {
    type: "detailResult",
    requestId: detailReq.id,
    result: { ok: true, value: A_DETAIL },
  });
  const profileDone = update(detailDone.model, {
    type: "profileResult",
    requestId: profileReq.id,
    result: { ok: true, value: { status: "missing" } },
  });
  assert.equal(profileDone.model.saved.detail.value, null);
  const bDetail = effect(detailDone.effects, "getInvocation");
  const bProfile = effect(profileDone.effects, "getProfile");
  assert.equal(bDetail.workspace, OTHER);
  assert.equal(bDetail.invocationId, "b-id");
  assert.equal(bProfile.workspace, OTHER);
  assert.equal(bProfile.invocationId, "b-id");
  const loaded = update(profileDone.model, {
    type: "detailResult",
    requestId: bDetail.id,
    result: { ok: true, value: B_DETAIL },
  });
  assert.deepEqual(loaded.model.saved.detail.value, B_DETAIL);
});

test("reopening a workspace with a changed scope refreshes exactly once", () => {
  for (const eventFirst of [true, false]) {
    const initial = state({ revision: 2 }, [
      wsEntry(WS, { scope: { include: ["//app/..."] }, snapshotFetchedAt: 1 }),
    ]);
    const model = connectedModel(initial);
    const scoped = wsEntry(WS, {
      scope: { include: ["//other/..."] },
      snapshotFetchedAt: null,
    });
    const pushed = state({ revision: 3 }, [scoped]);
    let refreshes = 0;
    let current = model;
    if (eventFirst) {
      const pushedStep = update(current, { type: "appState", state: pushed });
      refreshes += pushedStep.effects.filter((item) => item.type === "refresh").length;
      current = pushedStep.model;
    }
    const reply = openReply(current, WS, { include: ["//other/..."] }, pushed);
    refreshes += reply.effects.filter((item) => item.type === "refresh").length;
    // The ingest-running push after the reply must not refresh again.
    const running = state({ revision: 4 }, [
      wsEntry(WS, {
        scope: { include: ["//other/..."] },
        snapshotFetchedAt: null,
        ingest: "running",
      }),
    ]);
    const after = update(reply.model, { type: "appState", state: running });
    refreshes += after.effects.filter((item) => item.type === "refresh").length;
    assert.equal(refreshes, 1, `eventFirst=${eventFirst}`);
  }
});

test("refreshMissingTargets skips running, errored, and populated workspaces", () => {
  const cases: Array<[Partial<WorkspaceState>, number]> = [
    [{ snapshotFetchedAt: null, ingest: "running" }, 0],
    [{ snapshotFetchedAt: null, lastError: { exitCode: 1, summary: "boom" } }, 0],
    [{ snapshotFetchedAt: 7 }, 0],
    [{ snapshotFetchedAt: null }, 1],
  ];
  for (const [over, expected] of cases) {
    const model = connectedModel(state());
    const opened = update(model, {
      type: "openWorkspace",
      path: OTHER,
      scope: { include: ["//..."] },
    });
    const reply = update(opened.model, {
      type: "workspaceResult",
      requestId: opened.model.workspacePrompt.requestId!,
      result: {
        ok: true,
        value: {
          workspace: OTHER,
          state: state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER, over)]),
        },
      },
    });
    assert.equal(
      reply.effects.filter((item) => item.type === "refresh").length,
      expected,
      JSON.stringify(over),
    );
  }
});

test("stale open replies cannot alter the prompt or selection", () => {
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER)]));
  const first = update(model, {
    type: "openWorkspace",
    path: OTHER,
    scope: { include: ["//a"] },
  });
  const second = update(first.model, {
    type: "openWorkspace",
    path: "/third",
    scope: { include: ["//b"] },
  });
  const idA = first.model.workspacePrompt.requestId!;
  const idB = second.model.workspacePrompt.requestId!;
  // A late success for A still applies its aggregate but not the selection.
  const lateA = update(second.model, {
    type: "workspaceResult",
    requestId: idA,
    result: {
      ok: true,
      value: { workspace: OTHER, state: state({ revision: 5 }) },
    },
  });
  assert.equal(lateA.model.workspace, WS);
  assert.equal(lateA.model.workspacePrompt.status, "opening");
  assert.equal(lateA.model.connection.state?.revision, 5);
  // A late error for A changes nothing.
  const lateErr = update(lateA.model, {
    type: "workspaceResult",
    requestId: idA,
    result: { ok: false, error: "stale failure" },
  });
  assert.equal(lateErr.model.workspacePrompt.status, "opening");
  assert.equal(lateErr.model.workspacePrompt.error, null);
  // B succeeds, then A's error still cannot reopen the prompt.
  const bOk = update(lateErr.model, {
    type: "workspaceResult",
    requestId: idB,
    result: {
      ok: true,
      value: { workspace: "/third", state: state({ revision: 6 }) },
    },
  });
  assert.equal(bOk.model.workspace, "/third");
  assert.equal(bOk.model.workspacePrompt.status, "closed");
  const afterErr = update(bOk.model, {
    type: "workspaceResult",
    requestId: idA,
    result: { ok: false, error: "stale failure" },
  });
  assert.equal(afterErr.model.workspacePrompt.status, "closed");
  assert.equal(afterErr.model.workspacePrompt.error, null);
});

test("a rejected start after same-workspace navigation shows no stale error", () => {
  const model = indexedModel();
  const started = update(model, {
    type: "start",
    cmd: "build",
    pattern: "//app:first",
    argumentsText: "",
  });
  const request = effect(started.effects, "startInvocation");
  const navigated = update(started.model, { type: "showPage", page: "query" });
  const failed = update(navigated.model, {
    type: "startResult",
    requestId: request.id,
    result: { ok: false, error: "spawn failed" },
  });
  assert.equal(failed.model.connection.error, null);
});

test("connection readiness derives from state; failed polls keep it", () => {
  const failed = update(initialModel(), {
    type: "stateResult",
    result: { ok: false, error: "ipc down" },
  });
  assert.equal(failed.model.connection.state, null);
  assert.equal(sessionView(failed.model).ready, false);
  assert.equal(failed.model.connection.error, "Could not connect: ipc down");
  const connected = update(failed.model, {
    type: "appState",
    state: state({ revision: 1 }),
  });
  assert.equal(sessionView(connected.model).ready, true);
  assert.equal(connected.model.connection.error, null);
  // A failed poll after connecting keeps state, readiness, and error.
  const pollFail = update(connected.model, {
    type: "stateResult",
    result: { ok: false, error: "ipc down" },
  });
  assert.equal(pollFail.model.connection.state?.revision, 1);
  assert.equal(sessionView(pollFail.model).ready, true);
  assert.equal(pollFail.model.connection.error, null);
  // A subscriptions failure reports the error without touching state.
  const subFail = update(connected.model, {
    type: "subscriptionsFailed",
    error: "listen failed",
  });
  assert.equal(subFail.model.connection.state?.revision, 1);
  assert.equal(subFail.model.connection.error, "Could not connect: listen failed");
});

test("pending starts stay per-request while another workspace navigates", () => {
  const model = indexedModel(state({}, [wsEntry(WS), wsEntry(OTHER)]));
  const startA = update(model, {
    type: "start",
    cmd: "build",
    pattern: "//app:first",
    argumentsText: "",
  });
  const reqA = effect(startA.effects, "startInvocation");
  assert.equal(reqA.workspace, WS);
  const opened = update(startA.model, {
    type: "openWorkspace",
    path: OTHER,
    scope: { include: ["//..."] },
  });
  const switched = update(opened.model, {
    type: "workspaceResult",
    requestId: opened.model.workspacePrompt.requestId!,
    result: {
      ok: true,
      value: {
        workspace: OTHER,
        state: state({ revision: 2 }, [wsEntry(WS), wsEntry(OTHER)]),
      },
    },
  });
  assert.equal(switched.model.workspace, OTHER);
  // A's pending start survives the switch; its late reply cannot navigate.
  assert.equal(switched.model.starts[reqA.id]?.workspace, WS);
  const lateA = update(switched.model, {
    type: "startResult",
    requestId: reqA.id,
    result: { ok: true, value: owned("inv-a", WS) },
  });
  assert.deepEqual(lateA.model.content, { kind: "targets" });
  assert.equal(lateA.model.starts[reqA.id], undefined);
  // B's own start is unaffected and still navigates on its reply.
  const startB = update(lateA.model, {
    type: "start",
    cmd: "build",
    pattern: "//app:first",
    argumentsText: "",
  });
  const reqB = effect(startB.effects, "startInvocation");
  assert.equal(reqB.workspace, OTHER);
  const repliedB = update(startB.model, {
    type: "startResult",
    requestId: reqB.id,
    result: { ok: true, value: owned("inv-b", OTHER) },
  });
  assert.deepEqual(repliedB.model.content, {
    kind: "console",
    id: "inv-b",
    page: "targets",
  });
  assert.equal(repliedB.model.starts[reqB.id], undefined);
});

test("the console display record is the owned entry itself, with no invented exit", () => {
  const live = owned("inv-1", WS, { running: true });
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS)], [live]));
  const opened = update(model, { type: "openConsole", id: "inv-1" });
  const view = sessionView(opened.model);
  assert.equal(view.console?.exitCode, undefined);
  assert.equal(view.console, live);
});

test("a completed console-less query keeps navigation and offers no stop or reads", () => {
  const query = owned("q-1", WS, { cmd: "query", console: false });
  const model = connectedModel(state({ revision: 2 }, [wsEntry(WS)], [query]));
  const selected = update(model, { type: "selectInvocation", id: "q-1" });
  assert.deepEqual(selected.model.content, {
    kind: "invocation",
    id: "q-1",
    page: "targets",
  });
  // The core prunes a finished console-less query: the owned entry is gone.
  const finished = update(selected.model, {
    type: "appState",
    state: state({ revision: 3 }, [wsEntry(WS)], []),
  });
  const view = sessionView(finished.model);
  assert.equal(view.execution, null);
  assert.deepEqual(finished.model.content, {
    kind: "invocation",
    id: "q-1",
    page: "targets",
  });
  // No BEP record exists, so no artifact read is attempted.
  assert.equal(
    finished.effects.some((item) => item.type === "getInvocation" || item.type === "getProfile"),
    false,
  );
  // Nothing owned remains to stop.
  const stopped = update(finished.model, { type: "stop", id: "q-1" });
  assert.equal(stopped.effects.length, 0);
});

test("owned execution changes do not refetch history; historyChanged still coalesces", () => {
  const model = connectedModel(state({ revision: 2 }));
  const settled = settleHistory(model);
  const pushed = update(settled, {
    type: "appState",
    state: state({ revision: 3 }, [wsEntry(WS)], [owned("q-1", WS, { cmd: "query" })]),
  });
  assert.equal(
    pushed.effects.some((item) => item.type === "listInvocations"),
    false,
  );
  const changed = update(pushed.model, { type: "historyChanged" });
  assert.equal(effect(changed.effects, "listInvocations").workspace, WS);
  // While that read is in flight, another notification coalesces to one follow-up.
  const again = update(changed.model, { type: "historyChanged" });
  assert.equal(again.effects.filter((item) => item.type === "listInvocations").length, 0);
  const landed = update(again.model, {
    type: "invocationsResult",
    requestId: effect(changed.effects, "listInvocations").id,
    result: { ok: true, value: [] },
  });
  assert.equal(landed.effects.filter((item) => item.type === "listInvocations").length, 1);
});
