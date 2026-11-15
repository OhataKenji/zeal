import { test } from "node:test";
import assert from "node:assert/strict";
import type { AppState, TargetIndex, WorkspaceState } from "../src/lib/types.ts";
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

test("App renders workspace navigation and invocation history", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { app } = await load("/src/lib/state.svelte.ts");
  const { default: App } = await load("/src/App.svelte");

  function output(startedAt: number | undefined): string {
    const record = { id: "invocation", cmd: "build", pattern: "//app:app", startedAt };
    Object.assign(app.view, {
      ready: true,
      core: CORE,
      state: WS,
      invocations: [record],
      invocationRows: [{ record }],
    });
    return render(App).body;
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

  await t.test("names workspace navigation and command controls accessibly", () => {
    const html = output(1_700_000_000_000);
    const nav = html.match(/<nav[^>]*aria-label="Workspace views"[^>]*>([\s\S]*?)<\/nav>/)?.[1];
    assert.ok(nav, "workspace navigation is missing");
    assert.match(nav, /<button[^>]*>(?:(?!<\/button>)[\s\S])*?Query<\/button>/);
    assert.match(nav, /<button[^>]*>(?:(?!<\/button>)[\s\S])*?Analytics<\/button>/);
    assert.match(html, /<button[^>]*aria-label="Build"/);
    assert.match(html, /<button[^>]*aria-label="Reload targets"/);
    const toggle = html.match(/<button[^>]*aria-label="Arguments"[^>]*>/)?.[0];
    assert.ok(toggle, "arguments toggle is missing");
    assert.match(toggle, /aria-expanded="false"/);
    assert.doesNotMatch(html, /id="command-arguments"/);
  });
});

test("App shows snapshot errors alongside retained targets", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { app } = await load("/src/lib/state.svelte.ts");
  const { default: App } = await load("/src/App.svelte");

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
    Object.assign(app.view, {
      ready: true,
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
    return render(App).body;
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

test("App shows a pending Console placeholder before the owned entry arrives", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { app } = await load("/src/lib/state.svelte.ts");
  const { default: App } = await load("/src/App.svelte");

  Object.assign(app.view, {
    ready: true,
    core: CORE,
    state: WS,
    content: { kind: "console", id: "pending", page: "targets" },
    console: null,
    execution: null,
    invocations: [],
    invocationRows: [],
    invocationsError: null,
  });
  const html = render(App).body;
  assert.ok(html.includes('role="status"'));
  assert.ok(html.includes("Waiting for Console…"));
});
