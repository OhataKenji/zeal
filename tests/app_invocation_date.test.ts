import { test } from "node:test";
import assert from "node:assert/strict";
import { invocationStatus } from "../src/lib/invocation.ts";
import { createSession, emptyView } from "../src/lib/session.ts";
import type { AppState, OwnedInvocation, TargetIndex, WorkspaceState } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

const WS: WorkspaceState = {
  workspace: "/workspace",
  scope: { include: ["//app/..."] },
};

const CORE: AppState = {
  revision: 1,
  besError: null,
  besReady: true,
  bazel: "/bin/bazel",
  lastWorkspace: "/workspace",
  workspaces: [WS],
  invocations: [],
};

function renderSession() {
  return createSession(
    {
      invoke: async () => {
        throw new Error("render tests do not call the core");
      },
      listen: async () => () => {},
    },
    () => {},
  );
}

test("AppView renders workspace navigation and invocation history", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: AppView } = await load("/src/AppView.svelte");
  const session = renderSession();

  function output(startedAt: number | undefined): string {
    const record = { id: "invocation", cmd: "build", pattern: "//app:app", startedAt };
    const view = Object.assign(emptyView("history"), {
      core: CORE,
      state: WS,
      invocations: [record],
      invocationRows: [{ record }],
    });
    return render(AppView, { props: { view, session } }).body;
  }

  for (const startedAt of [undefined, NaN, Infinity, -Infinity, 8.64e15 + 1]) {
    await t.test(`${startedAt} renders the missing-time fallback`, () => {
      const html = output(startedAt);
      assert.ok(html.includes('<span class="invocation-time">Time not recorded</span>'));
      assert.doesNotMatch(html, /<time\b/);
    });
  }

  for (const [startedAt, expected] of [
    [0, "1970-01-01T00:00:00.000Z"],
    [1_700_000_000_000, "2023-11-14T22:13:20.000Z"],
    [8_640_000_000_000_000, "+275760-09-13T00:00:00.000Z"],
  ] as const) {
    await t.test(`${startedAt} renders a semantic time`, () => {
      const html = output(startedAt);
      assert.ok(html.includes(`<time class="invocation-time" datetime="${expected}"`));
      assert.doesNotMatch(html, /Time not recorded/);
    });
  }

  await t.test("names workspace rail domains accessibly", () => {
    const html = output(1_700_000_000_000);
    const nav = html.match(/<nav[^>]*aria-label="Workspace domains"[^>]*>([\s\S]*?)<\/nav>/)?.[1];
    assert.ok(nav, "workspace rail navigation is missing");
    assert.match(nav, /Workspace/);
    assert.match(nav, /Targets/);
    assert.match(nav, /Modules/);
    assert.match(nav, /Query/);
    assert.match(nav, /History/);
    assert.match(nav, /Analytics/);
  });

  await t.test("distinguishes simultaneous live rows by workspace", () => {
    const local: OwnedInvocation = {
      id: "local",
      workspace: WS.workspace,
      cmd: "run",
      pattern: "//app:app",
      startedAt: 1_700_000_000_000,
      console: true,
      stoppable: true,
      stopping: false,
    };
    const foreign = { ...local, id: "foreign", workspace: "/other/workspace" };
    const view = {
      ...emptyView("history"),
      core: CORE,
      state: WS,
      invocationRows: [
        { record: local, execution: local },
        { record: foreign, execution: foreign },
      ],
    };
    const html = render(AppView, { props: { view, session } }).body;
    assert.match(html, /aria-label="run \/\/app:app: ● running — \/workspace"/);
    assert.match(html, /aria-label="run \/\/app:app: ● running — \/other\/workspace"/);
    assert.match(html, />\/other\/workspace<\/small>/);
  });

  await t.test("shows the owning workspace next to the Stop control", async () => {
    const { default: InvocationHeader } = await load("/src/lib/InvocationHeader.svelte");
    const record = { id: "foreign", cmd: "run", pattern: "//app:app" };
    const execution: OwnedInvocation = {
      ...record,
      workspace: "/other/workspace",
      startedAt: 1_700_000_000_000,
      console: true,
      stoppable: true,
      stopping: false,
    };
    const html = render(InvocationHeader, {
      props: { record, status: invocationStatus(record, execution), execution, onstop: () => {} },
    }).body;
    assert.match(html, />Workspace \/other\/workspace<\/p>/);
    assert.match(html, /<button[^>]*aria-label="Stop in \/other\/workspace"/);
  });
});

test("AppView shows snapshot errors alongside retained targets", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: AppView } = await load("/src/AppView.svelte");
  const session = renderSession();

  const CACHED: TargetIndex = {
    meta: { fetchedAt: 1, status: "ok" },
    targets: [
      {
        label: "//:cached",
        pkg: "",
        name: "cached",
        ruleClass: "cc_library",
        location: null,
      },
    ],
  };

  function output(snapshot: TargetIndex | null, snapshotError: string | null): string {
    const view = Object.assign(emptyView("targets"), {
      core: CORE,
      state: WS,
      content: { kind: "targets" },
      selection: null,
      snapshot,
      snapshotError,
      snapshotLoading: false,
      targetDetail: null,
      targetDetailError: null,
      targetOutputs: null,
      targetOutputsError: null,
      targetActions: null,
      targetActionsError: null,
      invocations: [],
      invocationRows: [],
      invocationsError: null,
    });
    return render(AppView, { props: { view, session } }).body;
  }

  await t.test("a refresh error keeps the previous tree visible", () => {
    const html = output(CACHED, "refresh test failure");
    assert.ok(html.includes("refresh test failure"));
    assert.ok(html.includes("//:cached"));
  });

  await t.test("a partial snapshot reports its cause", () => {
    const html = output(
      { ...CACHED, meta: { fetchedAt: 2, status: "partial" } },
      "refresh test failure",
    );
    assert.ok(html.includes("Partial target list"));
    assert.ok(html.includes("refresh test failure"));
  });

  await t.test("an error without a snapshot uses an alert", () => {
    const html = output(null, "refresh test failure");
    assert.ok(html.includes('role="alert"'));
    assert.ok(html.includes("refresh test failure"));
    assert.ok(!html.includes("//:cached"));
  });
});

test("AppView shows lookup states without invocation tabs when the record is unavailable", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: AppView } = await load("/src/AppView.svelte");
  const session = renderSession();

  function output(over: Record<string, unknown>): string {
    const view = Object.assign(emptyView("history"), {
      core: CORE,
      state: WS,
      selectedInvocationId: "pending",
      invocations: [],
      invocationRows: [],
      invocationsError: null,
      detail: null,
      detailLoading: false,
      detailError: null,
      ...over,
    });
    return render(AppView, { props: { view, session } }).body;
  }

  await t.test("a missing record names the selected invocation", () => {
    const html = output({});
    assert.match(html, /No invocation record found for/);
    assert.match(html, /pending/);
    assert.doesNotMatch(html, /role="tablist" aria-label="Invocation views"/);
  });

  await t.test("history loading shows a pending state instead of not-found", () => {
    const html = output({ historyLoading: true });
    assert.match(html, /Loading invocation…/);
    assert.doesNotMatch(html, /No invocation record found/);
    assert.doesNotMatch(html, /role="tablist" aria-label="Invocation views"/);
  });

  await t.test("a failed history lookup surfaces the error", () => {
    const html = output({ invocationsError: "history unavailable" });
    assert.match(html, /history unavailable/);
    assert.doesNotMatch(html, /No invocation record found/);
    assert.doesNotMatch(html, /role="tablist" aria-label="Invocation views"/);
  });
});

test("AppView links history target labels into the target explorer", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: AppView } = await load("/src/AppView.svelte");
  const session = renderSession();
  const snapshot: TargetIndex = {
    meta: { fetchedAt: 0, status: "ok" },
    targets: [
      { label: "//app:x", pkg: "app", name: "x", ruleClass: "java_binary", location: null },
    ],
  };

  function output(workspace: string): string {
    const record = {
      id: "inv-1",
      workspace,
      cmd: "build",
      pattern: "//app:x",
      startedAt: 1_700_000_000_000,
      finishedAt: 1_700_000_001_000,
      exitCode: 0,
    };
    const view = Object.assign(emptyView("history"), {
      core: CORE,
      state: WS,
      selectedInvocationId: "inv-1",
      invocations: [record],
      invocationRows: [{ record }],
      invocation: { record },
      snapshot,
      detail: {
        record,
        log: { status: "missing" },
        bep: {
          status: "ok",
          data: {
            targets: [
              { label: "@@//app:x", ok: true },
              { label: "//app:missing", ok: false },
            ],
            tests: [{ label: "//app:x", status: "PASSED" }],
            truncated: false,
          },
        },
      },
    });
    return render(AppView, { props: { view, session } }).body.replace(/<!--[\s\S]*?-->/g, "");
  }

  const own = output("/workspace");
  assert.match(own, /<button[^>]*class="table-link mono"[^>]*>@@\/\/app:x<\/button>/);
  assert.match(own, /<button[^>]*class="table-link mono"[^>]*>\/\/app:x<\/button>/);
  assert.match(own, /<code>\/\/app:missing<\/code>/);
  assert.equal(own.match(/table-link/g)?.length, 2);

  const foreign = output("/other");
  assert.doesNotMatch(foreign, /table-link/);
  assert.match(foreign, /<code>@@\/\/app:x<\/code>/);
});
