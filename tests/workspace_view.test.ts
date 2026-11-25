import { test } from "node:test";
import assert from "node:assert/strict";
import type { WorkspaceInfo } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

const INFO: WorkspaceInfo = {
  version: "8.0.0",
  entries: [
    { key: "workspace", value: "/ws" },
    { key: "output_base", value: "/cache/output_base" },
  ],
};

test("WorkspaceView info rows offer per-value copy buttons", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: WorkspaceView } = await load("/src/lib/WorkspaceView.svelte");

  const html = render(WorkspaceView, {
    props: {
      workspace: "/ws",
      scope: { include: ["//..."] },
      info: INFO,
      loading: false,
      error: null,
      besEndpoint: null,
      onretry: () => {},
      onsavescope: async () => true,
    },
  }).body;

  // Rows are plain text; each value gets an icon button named by its key.
  assert.ok(html.includes('aria-label="Copy workspace value"'));
  assert.ok(html.includes('aria-label="Copy output_base value"'));
  assert.ok(html.includes('title="Copy workspace value"'));
  assert.doesNotMatch(html, /Click to copy/);
});

test("a Bazel info retry does not lock the scope editor", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: WorkspaceView } = await load("/src/lib/WorkspaceView.svelte");
  const html = render(WorkspaceView, {
    props: {
      workspace: "/ws",
      scope: { include: ["//app/..."] },
      info: null,
      loading: true,
      error: null,
      besEndpoint: null,
      onretry: () => {},
      onsavescope: async () => true,
    },
  }).body;

  assert.match(html, /Loading bazel info…/);
  assert.match(html, /<textarea\b(?![^>]*\bdisabled\b)[^>]*>/);
  assert.match(html, /<button\b(?![^>]*\bdisabled\b)[^>]*type="submit"[^>]*>/);
});
