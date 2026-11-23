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

test("BuildMetricsDetails renders the recorded BEP groups", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: BuildMetricsDetails } = await load(
    "/src/lib/invocation/BuildMetricsDetails.svelte",
  );

  function output(section: Section<Bep> | undefined, loading = false): string {
    return render(BuildMetricsDetails, { props: { section, loading } }).body.replace(
      /<!--[\s\S]*?-->/g,
      "",
    );
  }
  function ok(partial: Partial<Bep>): string {
    return output({ status: "ok", data: { ...EMPTY, ...partial } });
  }
  function groupText(html: string, title: string): string {
    const match = html.match(new RegExp(`<h4>${title}</h4>([\\s\\S]*?)</section>`));
    assert.ok(match, `missing group ${title}`);
    return visibleText(match[1]);
  }

  await t.test("all five groups render name/value rows", () => {
    const html = ok({
      localActionCache: {
        hits: 3,
        misses: 1,
        missReasons: [
          { name: "DIGEST_MISMATCH", count: 1 },
          { name: "NOT_CACHED", count: 0 },
        ],
      },
      outputArtifactsFromActionCache: { count: 7, sizeInBytes: "9007199254740993" },
      runnerCounts: [
        { name: "total", count: 9 },
        { name: "internal", count: 4 },
        { name: "darwin-sandbox", count: 0 },
      ],
      remoteAnalysisCache: { cacheHits: "9007199254740993", cacheMisses: "0" },
      tests: [{ label: "//pkg:t", status: "PASSED", totalNumCached: 2, totalRunCount: 24 }],
    });
    const text = visibleText(html);
    for (const title of [
      "Local action cache",
      "Output artifacts from action cache",
      "Runner count",
      "Test cache results",
      "Remote analysis cache",
    ]) {
      assert.ok(html.includes(`<h4>${title}</h4>`), `missing group ${title}`);
    }
    assert.equal(
      groupText(html, "Local action cache"),
      "Hits 3 Misses 1 DIGEST_MISMATCH 1 NOT_CACHED 0",
    );
    assert.equal(
      groupText(html, "Output artifacts from action cache"),
      "Count (files) 7 Size (bytes) 9007199254740993",
    );
    // Producer order is kept; the reported total is pinned last as Total.
    assert.equal(groupText(html, "Runner count"), "internal 4 darwin-sandbox 0 Total 9");
    assert.equal(
      groupText(html, "Test cache results"),
      "//pkg:t Cached test actions 2 Shard attempts 24",
    );
    assert.equal(
      groupText(html, "Remote analysis cache"),
      "Cache hits 9007199254740993 Cache misses 0",
    );
    assert.ok(!text.includes("Cache stats"));
  });

  await t.test("absent groups say Not recorded while zeros stay zero", () => {
    const empty = visibleText(ok({}));
    assert.equal((empty.match(/Not recorded/g) ?? []).length, 5);
    for (const title of [
      "Local action cache",
      "Output artifacts from action cache",
      "Runner count",
      "Test cache results",
      "Remote analysis cache",
    ]) {
      assert.ok(empty.includes(title), `missing group ${title}`);
    }

    const zeros = ok({
      localActionCache: { hits: 0, misses: 0, missReasons: [] },
      outputArtifactsFromActionCache: { count: 0, sizeInBytes: "0" },
      runnerCounts: [{ name: "internal", count: 0 }],
      remoteAnalysisCache: { cacheHits: "0", cacheMisses: "0" },
      tests: [{ label: "//pkg:t", status: "PASSED", totalNumCached: 0, totalRunCount: 0 }],
    });
    const text = visibleText(zeros);
    assert.match(groupText(zeros, "Local action cache"), /^Hits 0 Misses 0$/);
    assert.equal(
      groupText(zeros, "Output artifacts from action cache"),
      "Count (files) 0 Size (bytes) 0",
    );
    assert.equal(groupText(zeros, "Runner count"), "internal 0");
    assert.equal(
      groupText(zeros, "Test cache results"),
      "//pkg:t Cached test actions 0 Shard attempts 0",
    );
    assert.equal(groupText(zeros, "Remote analysis cache"), "Cache hits 0 Cache misses 0");
    assert.ok(!text.includes("Not recorded"));
  });

  await t.test("unrecorded test counts and repeated rows stay explicit", () => {
    const html = ok({
      tests: [{ label: "//pkg:t", status: "PASSED" }],
      runnerCounts: [
        { name: "internal", count: 1 },
        { name: "internal", count: 2 },
      ],
    });
    assert.equal(
      groupText(html, "Test cache results"),
      "//pkg:t Cached test actions Not recorded Shard attempts Not recorded",
    );
    assert.equal(groupText(html, "Runner count"), "internal 1 internal 2");
  });

  await t.test("loading and partial sections stay distinct", () => {
    assert.match(output(undefined, true), /Loading build metrics…/);
    assert.equal(output(undefined).includes("Loading"), false);
    const partial = output({
      status: "incomplete",
      data: { ...EMPTY, runnerCounts: [{ name: "internal", count: 3 }] },
      note: "saved prefix missing",
    });
    assert.match(partial, /incomplete — saved prefix missing/);
    assert.equal(groupText(partial, "Runner count"), "internal 3");
  });
});
