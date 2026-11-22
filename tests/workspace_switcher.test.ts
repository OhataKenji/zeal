import { test } from "node:test";
import assert from "node:assert/strict";
import { createSvelteHarness } from "./svelte.ts";

test("WorkspaceSwitcher renders a single trigger and split actions", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: WorkspaceSwitcher } = await load("/src/lib/WorkspaceSwitcher.svelte");

  function output(props: Record<string, unknown>): string {
    return render(WorkspaceSwitcher, {
      props: {
        workspace: "/work/cool-mono",
        opening: false,
        onswitch: () => {},
        onopennew: () => {},
        ...props,
      },
    })
      .body.replace(/<!--[\s\S]*?-->/g, "")
      .replace(/\s+/g, " ");
  }

  await t.test("one summary trigger carries the logo, basename, and full path title", () => {
    const html = output({});
    const triggers = html.match(/<summary[^>]*class="[^"]*ws-trigger[^"]*"[^>]*>/g) ?? [];
    assert.equal(triggers.length, 1);
    assert.match(triggers[0], /aria-label="Switch workspace"/);
    assert.match(triggers[0], /title="\/work\/cool-mono"/);
    const trigger = html.match(/<summary[^>]*>[\s\S]*?<\/summary>/)?.[0];
    assert.ok(trigger);
    assert.match(trigger, /<svg[^>]*aria-hidden="true"/);
    assert.match(trigger, />cool-mono</);
    assert.doesNotMatch(trigger, /cool-mono\//);
    assert.match(html, /Open workspace…/);
    // No legacy home button or second path row remains.
    assert.doesNotMatch(html, /ws-home|ws-path-row|Workspace home/);
  });

  await t.test("missing workspace falls back to the Zeal label", () => {
    const html = output({ workspace: null });
    const trigger = html.match(/<summary[^>]*>[\s\S]*?<\/summary>/)?.[0];
    assert.ok(trigger);
    assert.match(trigger, />Zeal</);
  });

  await t.test("opening disables the Open workspace footer", () => {
    const openingHtml = output({ opening: true });
    const openTag = openingHtml.match(/<button[^>]*class="[^"]*ws-open[^"]*"[^>]*>/)?.[0];
    assert.ok(openTag, "missing Open workspace button");
    assert.match(openTag, /disabled/);

    const html = output({});
    const enabledTag = html.match(/<button[^>]*class="[^"]*ws-open[^"]*"[^>]*>/)?.[0];
    assert.ok(enabledTag);
    assert.doesNotMatch(enabledTag, /disabled/);
    assert.doesNotMatch(html, /Wait for the current Bazel operation/);
  });

  await t.test("return mode is a single accessible button without a dropdown", () => {
    const html = output({ onreturn: () => {} });
    assert.doesNotMatch(html, /<details|<summary/);
    const button = html.match(/<button[^>]*aria-label="Return to workspace"[^>]*>/)?.[0];
    assert.ok(button, "missing return button");
    assert.match(button, /title="Return to workspace"/);
    assert.doesNotMatch(button, /\bdisabled\b/);
    assert.match(html, />cool-mono</);
  });

  await t.test("return mode disables the button while opening", () => {
    const html = output({ onreturn: () => {}, opening: true });
    const button = html.match(/<button[^>]*class="[^"]*ws-switch[^"]*ws-trigger[^"]*"[^>]*>/)?.[0];
    assert.ok(button, "missing return button");
    assert.match(button, /\bdisabled\b/);
  });
});
