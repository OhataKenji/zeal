import { test } from "node:test";
import assert from "node:assert/strict";
import type { Bep, Section } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

const EMPTY: Bep = { targets: [], tests: [], truncated: false };

function rowTexts(html: string): string[] {
  return Array.from(html.matchAll(/<div class="cache-row">([\s\S]*?)<\/div>/g), ([, content]) =>
    content
      .replace(/<[^>]*>/g, " ")
      .replace(/\s+/g, " ")
      .trim(),
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

  await t.test("local hits and misses show counts and hit rate", () => {
    const html = ok({
      localActionCache: { hits: 3, misses: 1, missReasons: [] },
      actionTypes: [],
      runnerTypes: [],
    });
    assert.match(html, /Local action cache/);
    assert.match(html, /Hits\s+3\s+·\s+Misses\s+1\s+·\s+75%\s+hit\s+rate/);
    assert.match(html, />75%</);
    assert.match(
      html,
      /Remote AC, CAS, and transfer statistics are not recorded in Bazel build events\./,
    );

    const dominated = ok({
      localActionCache: { hits: 1, misses: 3, missReasons: [] },
      actionTypes: [],
      runnerTypes: [],
    });
    assert.match(dominated, /Hits\s+1\s+·\s+Misses\s+3\s+·\s+25%\s+hit\s+rate/);
  });

  await t.test("breakdowns list named counts", () => {
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
      runnerTypes: [
        { name: "internal", count: 3 },
        { name: "darwin-sandbox", count: 2 },
      ],
    });
    for (const title of ["Local invalidation reasons", "Action types", "Runner types"]) {
      assert.ok(html.includes(title), `missing breakdown ${title}`);
    }
    assert.deepEqual(rowTexts(html), [
      "digest mismatch 2",
      "CppCompile 4",
      "CppLink 1",
      "internal 3",
      "darwin-sandbox 2",
    ]);
  });

  await t.test("long breakdowns show a bounded prefix", () => {
    const actionTypes = Array.from({ length: 12 }, (_, index) => ({
      name: `Action${index}`,
      count: index + 1,
    }));
    const html = ok({ actionTypes, runnerTypes: [] });
    assert.match(html, /Showing 10 of 12/);
    assert.ok(html.includes("Action9"));
    assert.ok(!html.includes("Action10"));
  });

  await t.test("missing stats differ from recorded zeros", () => {
    const absent = ok(undefined);
    assert.match(absent, /No cache statistics recorded in the build events\./);
    assert.ok(!absent.includes("Local action cache"));

    const zeros = ok({
      localActionCache: { hits: 0, misses: 0, missReasons: [] },
      actionTypes: [],
      runnerTypes: [],
    });
    assert.match(zeros, /Local action cache/);
    assert.match(zeros, /Hits\s+0\s+·\s+Misses\s+0/);
    // A zero denominator has no measurable rate, so no percentage renders.
    assert.ok(!zeros.includes("hit rate"));
    assert.ok(!zeros.includes("%"));

    const noLocal = ok({
      actionTypes: [{ name: "CppCompile", count: 2 }],
      runnerTypes: [],
    });
    assert.match(noLocal, /Not recorded in the build events\./);
    assert.match(noLocal, /Action types/);
    assert.ok(!noLocal.includes("hit rate"));
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
    assert.match(partial, /100%\s*hit\s+rate/);
  });
});
