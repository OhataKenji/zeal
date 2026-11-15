import { test } from "node:test";
import assert from "node:assert/strict";
import type { AppState } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

const CORE: AppState = {
  revision: 1,
  besError: null,
  besReady: true,
  bazel: "/bin/bazel",
  lastWorkspace: "/workspace",
  workspaces: [],
  invocations: [],
};

test("QueryView blocks Execute only on its own readiness checks", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { app } = await load("/src/lib/state.svelte.ts");
  const { default: QueryView } = await load("/src/lib/QueryView.svelte");

  function output(ready: boolean, core: AppState | null): string {
    Object.assign(app.view, { ready, core });
    return render(QueryView, {
      props: { snap: null, workspace: "/workspace" },
    }).body;
  }

  await t.test("a disconnected session disables Execute and explains why", () => {
    const html = output(false, CORE);
    assert.match(html, /class="button button-primary"[^>]*disabled/);
    assert.match(html, /not connected/);
  });

  await t.test("a missing Bazel disables Execute and explains why", () => {
    const html = output(true, { ...CORE, bazel: null });
    assert.match(html, /class="button button-primary"[^>]*disabled/);
    assert.match(html, /Bazel is not available/);
  });

  await t.test("a ready workspace leaves Execute enabled with no reason", () => {
    const html = output(true, CORE);
    assert.match(html, /class="button button-primary" title="Execute query"/);
    assert.doesNotMatch(html, /class="button button-primary"[^>]*disabled/);
    assert.doesNotMatch(html, /query-busy/);
    assert.equal(html.match(/<h1[\s>]/g)?.length, 1);
    assert.match(html, /<h1[^>]*id="query-heading"[^>]*>Query editor<\/h1>/);
    assert.doesNotMatch(html, /Bazel query/);
    assert.match(html, /aria-labelledby="query-heading"/);
  });
});
