import { test } from "node:test";
import assert from "node:assert/strict";
import { createSvelteHarness } from "./svelte.ts";
import {
  defaultQueryDraft,
  emptyQueryState,
  makeCall,
  target,
  validate,
  type QueryDraft,
  type QueryState,
} from "../src/lib/query.ts";
import { createIndex } from "../src/lib/search.ts";

test("QueryView blocks Execute only on Bazel availability", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: QueryView } = await load("/src/lib/QueryView.svelte");

  function output(bazelAvailable: boolean): string {
    return render(QueryView, {
      props: {
        targetIndex: null,
        bazelAvailable,
        query: emptyQueryState(),
        initialDraft: defaultQueryDraft(),
      },
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
    for (const kind of ["query", "cquery", "aquery"]) {
      assert.match(html, new RegExp(`role="tab"[^>]*>${kind}</button>`));
    }
  });
});

test("QueryView restores draft and links known workspace targets in query and cquery results", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: QueryView } = await load("/src/lib/QueryView.svelte");
  const targetIndex = createIndex([
    { label: "//app:server", ruleClass: "cc_binary", pkg: "app", name: "server", location: null },
    { label: "//lib:util", ruleClass: "cc_library", pkg: "lib", name: "util", location: null },
  ]);

  await t.test("query raw output links known targets and keeps external labels plain", () => {
    const draft: QueryDraft = {
      ...defaultQueryDraft(),
      mode: "code",
      draft: "rdeps(//..., //lib:util)",
    };
    const query: QueryState = {
      running: false,
      error: null,
      result: {
        exitCode: 0,
        stdout: "//app:server\n@@//lib:util\n@rules_go//go:def.bzl\n",
        stdoutTruncated: false,
        truncated: false,
        stderrTail: "",
        actions: null,
      },
    };
    const html = render(QueryView, {
      props: {
        targetIndex,
        bazelAvailable: true,
        query,
        initialDraft: draft,
        onselecttarget: () => {},
      },
    }).body.replace(/<!--[\s\S]*?-->/g, "");
    assert.match(html, /rdeps\(\/\/\.\.\., \/\/lib:util\)/);
    assert.match(
      html,
      /<button[^>]*class="table-link mono query-raw-link"[^>]*>\/\/app:server<\/button>/,
    );
    assert.match(
      html,
      /<button[^>]*class="table-link mono query-raw-link"[^>]*>@@\/\/lib:util<\/button>/,
    );
    assert.doesNotMatch(html, /<button[^>]*>@rules_go\/\/go:def\.bzl<\/button>/);
    assert.ok(html.includes("@rules_go//go:def.bzl"));
  });

  await t.test("cquery table links known targets and falls back to code for unknown ones", () => {
    const draft: QueryDraft = {
      ...defaultQueryDraft(),
      kind: "cquery",
      mode: "code",
      draft: "deps(//app:server)",
      buildFlags: "--config=opt",
      resultView: "table",
    };
    const query: QueryState = {
      running: false,
      error: null,
      result: {
        exitCode: 0,
        stdout: "//app:server (abc123)\n@ext//pkg:lib (def456)\n",
        stdoutTruncated: false,
        truncated: false,
        stderrTail: "",
        actions: null,
      },
    };
    const html = render(QueryView, {
      props: {
        targetIndex,
        bazelAvailable: true,
        query,
        initialDraft: draft,
        onselecttarget: () => {},
      },
    }).body.replace(/<!--[\s\S]*?-->/g, "");
    assert.match(html, /<button[^>]*class="table-link mono"[^>]*>\/\/app:server<\/button>/);
    assert.ok(html.includes("<code>@ext//pkg:lib</code>"));
    assert.ok(html.includes("--config=opt"));
  });
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
