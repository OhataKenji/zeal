import { test } from "node:test";
import assert from "node:assert/strict";
import { createSvelteHarness } from "./svelte.ts";

test("QueryView blocks Execute only on Bazel availability", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: QueryView } = await load("/src/lib/QueryView.svelte");

  function output(bazelAvailable: boolean): string {
    return render(QueryView, {
      props: { snap: null, workspace: "/workspace", bazelAvailable },
    }).body;
  }

  await t.test("missing Bazel disables Execute and explains why", () => {
    const html = output(false);
    assert.match(html, /class="button button-primary"[^>]*disabled/);
    assert.match(html, /Bazel is not available/);
  });

  await t.test("available Bazel leaves Execute enabled with no reason", () => {
    const html = output(true);
    assert.match(html, /class="button button-primary" title="Execute query"/);
    assert.doesNotMatch(html, /class="button button-primary"[^>]*disabled/);
    assert.doesNotMatch(html, /query-busy/);
    assert.equal(html.match(/<h1[\s>]/g)?.length, 1);
    assert.match(html, /<h1[^>]*id="query-heading"[^>]*>Query editor<\/h1>/);
    assert.doesNotMatch(html, /Bazel query/);
    assert.match(html, /aria-labelledby="query-heading"/);
  });
});
