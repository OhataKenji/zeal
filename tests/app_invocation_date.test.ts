import { test } from "node:test";
import assert from "node:assert/strict";
import type { AppState, WorkspaceState } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

const WS: WorkspaceState = {
  workspace: "/workspace",
  scope: { include: ["//app/..."] },
  ingest: "idle",
  snapshotFetchedAt: null,
  lastError: null,
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
