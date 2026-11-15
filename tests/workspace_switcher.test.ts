import { test } from "node:test";
import assert from "node:assert/strict";
import { createSvelteHarness } from "./svelte.ts";

test("WorkspaceSwitcher renders home, switch, and settings as separate controls", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: WorkspaceSwitcher } = await load("/src/lib/WorkspaceSwitcher.svelte");

  function output(props: Record<string, unknown>): string {
    return render(WorkspaceSwitcher, {
      props: {
        workspace: "/work/cool-mono",
        settingsOpen: false,
        opening: false,
        onopen: () => {},
        onsettings: () => {},
        onhome: () => {},
        ...props,
      },
    })
      .body.replace(/<!--[\s\S]*?-->/g, "")
      .replace(/\s+/g, " ");
  }

  function buttonTag(html: string, label: string): string {
    const match = html.match(new RegExp(`<button[^>]*aria-label="${label}"[^>]*>`));
    assert.ok(match, `missing button labeled ${label}`);
    return match[0];
  }

  await t.test("shows separately named home, switch, and settings controls", () => {
    const html = output({});
    assert.doesNotMatch(buttonTag(html, "Workspace home"), /disabled/);
    assert.match(buttonTag(html, "Workspace settings"), /aria-pressed="false"/);
    assert.doesNotMatch(buttonTag(html, "Workspace settings"), /disabled/);
    assert.match(html, /<summary[^>]*aria-label="Switch workspace"/);
    assert.match(html, />\/work\/cool-mono</);
    assert.match(html, /Open workspace…/);
  });

  await t.test("name shows the local basename even with a trailing separator", () => {
    const html = output({ workspace: "/work/cool-mono/" });
    const home = html.match(/<button[^>]*aria-label="Workspace home"[^>]*>[\s\S]*?<\/button>/);
    assert.ok(home, "missing workspace home button");
    assert.match(home[0], />cool-mono</);
    assert.doesNotMatch(home[0], /cool-mono\//);
    assert.match(home[0], /<svg[^>]*aria-hidden="true"/);
  });

  await t.test("settings toggle reflects open state via aria-pressed", () => {
    const html = output({ settingsOpen: true });
    assert.match(buttonTag(html, "Workspace settings"), /aria-pressed="true"/);
  });

  await t.test("opening disables Open; default Open is enabled without a busy notice", () => {
    const openingHtml = output({ opening: true });
    assert.match(openingHtml, /class="[^"]*ws-open[^"]*"[^>]*disabled/);

    const html = output({});
    const openTag = html.match(/<button[^>]*class="[^"]*ws-open[^"]*"[^>]*>/)?.[0];
    assert.ok(openTag, "missing Open workspace button");
    assert.doesNotMatch(openTag, /disabled/);
    assert.doesNotMatch(html, /Wait for the current Bazel operation/);
  });

  await t.test("opening and missing workspace disable home and settings", () => {
    const openingHtml = output({ opening: true });
    assert.match(buttonTag(openingHtml, "Workspace home"), /disabled/);
    assert.match(buttonTag(openingHtml, "Workspace settings"), /disabled/);

    const noWorkspace = output({ workspace: null });
    assert.match(buttonTag(noWorkspace, "Workspace home"), /disabled/);
    assert.match(buttonTag(noWorkspace, "Workspace settings"), /disabled/);
  });
});
