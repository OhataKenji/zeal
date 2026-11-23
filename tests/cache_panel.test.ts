import { test } from "node:test";
import assert from "node:assert/strict";
import type { Bep, Section } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

const EMPTY: Bep = { targets: [], tests: [], truncated: false };

function visibleText(html: string): string {
  return html
    .replace(/<[^>]*>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function legendItems(html: string): string[] {
  return Array.from(
    html.matchAll(/<li class="cache-legend-item">([\s\S]*?)<\/li>/g),
    ([, content]) => visibleText(content),
  );
}

test("CachePanel renders BEP cache stats", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: CachePanel } = await load("/src/lib/invocation/CachePanel.svelte");

  function output(section: Section<Bep> | undefined, loading = false): string {
    return render(CachePanel, { props: { section, loading } }).body.replace(/<!--[\s\S]*?-->/g, "");
  }
  function ok(cacheStats: Bep["cacheStats"]): string {
    return output({ status: "ok", data: { ...EMPTY, cacheStats } });
  }

  await t.test("local hits and misses keep fixed order with shares", () => {
    const html = ok({
      localActionCache: { hits: 3, misses: 1, missReasons: [] },
      actionTypes: [],
      runnerTypes: [],
    });
    const text = visibleText(html);
    assert.match(html, />Cache stats<\/h3>/);
    assert.match(html, />Local action cache<\/h3>/);
    assert.match(text, /Lookups in Bazel’s local action cache/);
    assert.deepEqual(legendItems(html), ["3 hits (75%)", "1 miss (25%)"]);
    // The remote-limitation note is a footnote below the data.
    assert.ok(text.indexOf("Local action cache") < text.indexOf("Remote AC, CAS"));

    const dominated = visibleText(
      ok({
        localActionCache: { hits: 1, misses: 3, missReasons: [] },
        actionTypes: [],
        runnerTypes: [],
      }),
    );
    assert.match(dominated, /1 hit \(25%\) 3 misses \(75%\)/);
    const dominatedHtml = ok({
      localActionCache: { hits: 1, misses: 3, missReasons: [] },
      actionTypes: [],
      runnerTypes: [],
    });
    assert.deepEqual(legendItems(dominatedHtml), ["1 hit (25%)", "3 misses (75%)"]);
  });

  await t.test("zero lookups show a neutral chart with real zeros", () => {
    const html = ok({
      localActionCache: { hits: 0, misses: 0, missReasons: [] },
      actionTypes: [],
      runnerTypes: [],
    });
    const text = visibleText(html);
    assert.match(html, />Local action cache<\/h3>/);
    assert.match(text, /No lookups/);
    assert.deepEqual(legendItems(html), ["0 hits", "0 misses"]);
    assert.ok(!text.includes("%"));
  });

  await t.test("runner-only stats lead with Runner types", () => {
    const html = ok({
      actionTypes: [{ name: "CppCompile", count: 4 }],
      runnerTypes: [
        { name: "internal", count: 8 },
        { name: "darwin-sandbox", count: 10 },
      ],
    });
    const text = visibleText(html);
    assert.ok(!html.includes(">Local action cache</h3>"));
    assert.match(html, />Runner types<\/h3>/);
    const runner = html.indexOf(">Runner types</h3>");
    const action = html.indexOf(">Action types</h3>");
    assert.ok(runner !== -1 && action !== -1 && runner < action);
    // Descending counts: the larger runner leads the legend.
    assert.deepEqual(legendItems(html), [
      "10 darwin-sandbox (56%)",
      "8 internal (44%)",
      "4 CppCompile (100%)",
    ]);
    assert.match(
      text,
      /Local action cache was not recorded\. Remote AC, CAS, and transfer statistics are not recorded in Bazel build events\./,
    );
    assert.ok(text.indexOf("darwin-sandbox") < text.indexOf("Remote AC, CAS"));
  });

  await t.test("breakdowns keep invalidation reasons and fold long tails", () => {
    const html = ok({
      localActionCache: {
        hits: 2,
        misses: 2,
        missReasons: [{ name: "digest mismatch", count: 2 }],
      },
      actionTypes: [
        { name: "CppCompile", count: 4 },
        { name: "CppLink", count: 1 },
      ],
      runnerTypes: [{ name: "internal", count: 3 }],
    });
    for (const title of ["Runner types", "Action types", "Local invalidation reasons"]) {
      assert.ok(html.includes(title), `missing breakdown ${title}`);
    }
    assert.deepEqual(legendItems(html), [
      "2 hits (50%)",
      "2 misses (50%)",
      "3 internal (100%)",
      "4 CppCompile (80%)",
      "1 CppLink (20%)",
      "2 digest mismatch (100%)",
    ]);

    // Twelve categories collapse to the five largest plus a summed Other.
    const many = ok({
      actionTypes: Array.from({ length: 12 }, (_, index) => ({
        name: `Action${index}`,
        count: index + 1,
      })),
      runnerTypes: [],
    });
    const items = legendItems(many);
    assert.equal(items.length, 6);
    assert.deepEqual(items.slice(0, 5), [
      "12 Action11 (15%)",
      "11 Action10 (14%)",
      "10 Action9 (13%)",
      "9 Action8 (12%)",
      "8 Action7 (10%)",
    ]);
    assert.equal(items[5], "28 Other (36%)");
    assert.ok(!many.includes("Action6"));
  });

  await t.test("missing stats render an empty note", () => {
    const absent = ok(undefined);
    assert.match(absent, /No cache statistics recorded in the build events\./);
    assert.ok(!absent.includes("Runner types"));
    assert.ok(!absent.includes("Remote AC, CAS"));
  });

  await t.test("loading and partial sections stay distinct", () => {
    assert.match(output(undefined, true), /Loading cache statistics…/);
    assert.equal(output(undefined).includes("Loading"), false);
    const partial = output({
      status: "incomplete",
      data: {
        ...EMPTY,
        cacheStats: {
          localActionCache: { hits: 1, misses: 0, missReasons: [] },
          actionTypes: [],
          runnerTypes: [],
        },
      },
      note: "saved prefix missing",
    });
    assert.match(partial, /incomplete — saved prefix missing/);
    assert.match(visibleText(partial), /1 hit \(100%\)/);
  });
});
