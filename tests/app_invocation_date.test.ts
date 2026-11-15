import { test } from "node:test";
import assert from "node:assert/strict";
import type { AppState } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

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
  detailQuery: "idle",
};

test("App renders workspace navigation and invocation history", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { app } = await load("/src/lib/state.svelte.ts");
  const { default: App } = await load("/src/App.svelte");

  function output(startedAt: number | undefined): string {
    Object.assign(app.view, {
      ready: true,
      state: IDLE,
      invocations: [{ id: "invocation", cmd: "build", pattern: "//app:app", startedAt }],
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

  await t.test("renders the workspace nav and an accessible icon action", () => {
    const html = output(1_700_000_000_000);
    assert.match(html, /<nav class="workspace-nav" aria-label="Workspace views">/);
    assert.match(
      html,
      /<button[^>]*class="[^"]*workspace-nav-link[^"]*"[^>]*>(?:(?!<\/button>)[\s\S])*?Query<\/button>/,
    );
    assert.match(
      html,
      /<button[^>]*class="[^"]*workspace-nav-link[^"]*"[^>]*>(?:(?!<\/button>)[\s\S])*?Analytics<\/button>/,
    );
    const action = html.match(
      /<button(?=[^>]*class="[^"]*button-primary[^"]*")(?=[^>]*aria-label="Build")[^>]*>[\s\S]*?<\/button>/,
    )?.[0];
    assert.ok(action, "primary action button is missing");
    assert.match(action, /title="Build"/);
    assert.match(action, /icon-button/);
    assert.match(action, /<svg/);
    assert.equal(action.replace(/<[^>]*>/g, "").trim(), "");
  });

  await t.test("renders an icon-only arguments toggle", () => {
    const html = output(1_700_000_000_000);
    const toggle = html.match(
      /<button(?=[^>]*aria-label="Arguments")[^>]*>[\s\S]*?<\/button>/,
    )?.[0];
    assert.ok(toggle, "arguments toggle is missing");
    assert.match(toggle, /aria-expanded="false"/);
    assert.match(toggle, /<svg/);
    assert.equal(toggle.replace(/<[^>]*>/g, "").trim(), "");
    assert.ok(!html.includes('id="command-arguments"'));
    assert.ok(!html.includes("arguments-indicator"));
  });

  await t.test("renders the targets toolbar without a count", () => {
    const html = output(1_700_000_000_000);
    assert.ok(!html.includes("pane-count"));
    const toolbar = html.match(/<div class="pane-toolbar">([\s\S]*?)<\/div>/)?.[1];
    assert.ok(toolbar, "targets toolbar is missing");
    const buttons = toolbar.match(/<button[\s\S]*?<\/button>/g) ?? [];
    assert.equal(buttons.length, 2);
    assert.match(buttons[0], /aria-expanded="(?:true|false)"/);
    assert.ok(buttons[0].includes("Targets"));
    assert.match(buttons[1], /aria-label="Reload targets"/);
  });
});
