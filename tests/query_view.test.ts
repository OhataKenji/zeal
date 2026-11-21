import { test } from "node:test";
import assert from "node:assert/strict";
import { createSvelteHarness } from "./svelte.ts";
import { makeCall, target, validate } from "../src/lib/query.ts";

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

test("configured graph presets fill the editor without executing and restrict its controls", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: QueryView } = await load("/src/lib/QueryView.svelte");
  for (const kind of ["cquery", "aquery"] as const) {
    const html = render(QueryView, {
      props: {
        snap: null,
        workspace: "/workspace",
        bazelAvailable: true,
        preset: { kind, flags: "--config=ci" },
      },
    }).body;
    assert.match(html, /Select Execute to fetch results/);
    assert.match(html, /aria-selected="true"[^>]*>Table</);
    assert.match(html, /--config=ci/);
    assert.doesNotMatch(html, />Graph<\/button>|--noimplicit_deps|value="buildfiles"/);
    if (kind === "cquery") {
      assert.match(html, /value="config"/);
      assert.match(html, /--show_config_fragments=off/);
      assert.doesNotMatch(html, /value="mnemonic"/);
    } else {
      assert.match(html, /value="mnemonic"/);
      assert.match(html, /--include_commandline --include_artifacts/);
      assert.doesNotMatch(html, /value="config"/);
    }
  }
});

test("a graph change keeps a disabled option and validation error on a nested unsupported card", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: QueryExpr } = await load("/src/lib/QueryExpr.svelte");
  const node = makeCall("deps", makeCall("buildfiles", target("//pkg:t")));
  const html = render(QueryExpr, {
    props: {
      node,
      queryKind: "cquery",
      errors: validate(node, "cquery"),
      searchIndex: null,
      kindIndex: null,
      attrIndex: null,
    },
  }).body;
  assert.match(html, /<option value="buildfiles"[^>]*disabled/);
  assert.match(html, /buildfiles is not available in cquery/);
  assert.doesNotMatch(html, /value="loadfiles"/);
});
