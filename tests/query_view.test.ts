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

test("QueryView reflects the target detail query's Bazel slot", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { app } = await load("/src/lib/state.svelte.ts");
  const { default: QueryView } = await load("/src/lib/QueryView.svelte");

  function output(detailQuery: AppState["detailQuery"]): string {
    Object.assign(app.view, {
      ready: true,
      state: { ...IDLE, detailQuery },
    });
    return render(QueryView, { props: { snap: null } }).body;
  }

  await t.test("a running detail query disables Execute and explains why", () => {
    const html = output("running");
    assert.match(html, /class="button button-primary"[^>]*disabled/);
    assert.match(html, /title="target details are loading"/);
    assert.match(html, /class="query-busy dim"[^>]*>\s*target details are loading\s*</);
  });

  await t.test("an idle detail query leaves Execute enabled with no reason", () => {
    const html = output("idle");
    assert.match(html, /class="button button-primary" title="Execute query"/);
    assert.doesNotMatch(html, /class="button button-primary"[^>]*disabled/);
    assert.doesNotMatch(html, /query-busy/);
    assert.equal(html.match(/<h1[\s>]/g)?.length, 1);
    assert.match(html, /<h1[^>]*id="query-heading"[^>]*>Query editor<\/h1>/);
    assert.doesNotMatch(html, /Bazel query/);
    assert.match(html, /aria-labelledby="query-heading"/);
  });
});
