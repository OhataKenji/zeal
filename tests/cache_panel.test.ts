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

function rowTexts(html: string): string[] {
  return Array.from(html.matchAll(/<li class="cache-row">([\s\S]*?)<\/li>/g), ([, content]) =>
    visibleText(content),
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

  await t.test("local hit rate leads with measured counts", () => {
    const html = ok({
      localActionCache: { hits: 3, misses: 1, missReasons: [] },
      actionTypes: [],
      runnerTypes: [],
    });
    const text = visibleText(html);
    assert.match(html, />Local action cache<\/h3>/);
    assert.match(text, /75% hit rate/);
    assert.match(text, /Hits 3 · Misses 1/);
    // The remote-limitation note is a footnote below the data.
    assert.ok(text.indexOf("Local action cache") < text.indexOf("Remote AC, CAS"));
    assert.doesNotMatch(text, /Cache stats/);

    const dominated = visibleText(
      ok({
        localActionCache: { hits: 1, misses: 3, missReasons: [] },
        actionTypes: [],
        runnerTypes: [],
      }),
    );
    assert.match(dominated, /25% hit rate/);
    assert.match(dominated, /Hits 1 · Misses 3/);
  });

  await t.test("zero lookups show no rate", () => {
    const html = ok({
      localActionCache: { hits: 0, misses: 0, missReasons: [] },
      actionTypes: [],
      runnerTypes: [],
    });
    const text = visibleText(html);
    assert.match(html, />Local action cache<\/h3>/);
    assert.match(text, /No lookups/);
    assert.match(text, /Hits 0 · Misses 0/);
    assert.ok(!text.includes("hit rate"));
    assert.ok(!text.includes("%"));
  });

  await t.test("runner-only stats lead with Runner types", () => {
    const html = ok({
      actionTypes: [{ name: "CppCompile", count: 4 }],
      runnerTypes: [
        { name: "internal", count: 3 },
        { name: "darwin-sandbox", count: 2 },
      ],
    });
    const text = visibleText(html);
    assert.ok(!html.includes(">Local action cache</h3>"));
    assert.match(html, />Runner types<\/h3>/);
    const runner = html.indexOf(">Runner types</h3>");
    const action = html.indexOf(">Action types</h3>");
    assert.ok(runner !== -1 && action !== -1 && runner < action);
    assert.deepEqual(rowTexts(html), ["internal 3", "darwin-sandbox 2", "CppCompile 4"]);
    assert.match(
      text,
      /Local action cache was not recorded\. Remote AC, CAS, and transfer statistics are not recorded in Bazel build events\./,
    );
    // Recorded data renders before the footnote.
    assert.ok(text.indexOf("darwin-sandbox") < text.indexOf("Remote AC, CAS"));
  });

  await t.test("breakdowns keep invalidation reasons and bound long lists", () => {
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
    assert.deepEqual(rowTexts(html), [
      "internal 3",
      "CppCompile 4",
      "CppLink 1",
      "digest mismatch 2",
    ]);

    const many = ok({
      actionTypes: Array.from({ length: 12 }, (_, index) => ({
        name: `Action${index}`,
        count: index + 1,
      })),
      runnerTypes: [],
    });
    assert.match(many, /Showing 10 of 12/);
    assert.ok(many.includes("Action9"));
    assert.ok(!many.includes("Action10"));
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
    assert.match(partial, />100%</);
    assert.match(partial, /hit rate/);
  });
});
