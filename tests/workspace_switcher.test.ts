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
        busy: false,
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

  await t.test("name, caret, path, and settings are separate controls", () => {
    const html = output({});
    const home = html.match(/<button[^>]*class="[^"]*ws-home[^"]*"[^>]*>[\s\S]*?<\/button>/);
    assert.ok(home, "missing workspace home button");
    assert.match(home[0], /aria-label="Workspace home"/);
    assert.match(home[0], /title="Workspace home"/);
    const logo = home[0].match(/<svg[^>]*class="[^"]*zeal-logo[^"]*"[^>]*>/)?.[0];
    assert.ok(logo, "workspace logo svg missing");
    assert.match(logo, /aria-hidden="true"/);
    assert.match(home[0], /cool-mono/);
    assert.doesNotMatch(home[0], /disabled/);

    const summary = html.match(/<summary[^>]*>[\s\S]*?<\/summary>/);
    assert.ok(summary, "missing workspace switch summary");
    assert.match(summary[0], /aria-label="Switch workspace"/);
    assert.match(summary[0], /title="Switch workspace"/);
    assert.doesNotMatch(summary[0], /cool-mono/);
    assert.doesNotMatch(summary[0], /\/work\/cool-mono/);

    const gear = buttonTag(html, "Workspace settings");
    assert.match(gear, /aria-pressed="false"/);
    assert.match(gear, /title="Workspace settings"/);
    assert.doesNotMatch(gear, /disabled/);

    assert.match(html, /class="[^"]*ws-path[^"]*"[^>]*title="\/work\/cool-mono"/);
    assert.match(html, />\/work\/cool-mono</);
    assert.match(html, /<details[^>]*class="[^"]*ws-details/);
    assert.match(html, /Open workspace…/);
  });

  await t.test("path row groups the path with the caret switcher", () => {
    const html = output({});
    const row = html.slice(html.indexOf("ws-path-row"));
    assert.ok(row.includes('title="/work/cool-mono"'));
    assert.ok(row.includes(">/work/cool-mono<"));
    assert.match(row, /<summary[^>]*class="[^"]*ws-summary/);
    assert.doesNotMatch(row, /ws-home/);
    assert.doesNotMatch(row, /Workspace settings/);
    assert.doesNotMatch(row, /Workspace home/);
  });

  await t.test("name shows the local basename even with a trailing separator", () => {
    const html = output({ workspace: "/work/cool-mono/" });
    const home = html.match(/<button[^>]*class="[^"]*ws-home[^"]*"[^>]*>[\s\S]*?<\/button>/);
    assert.ok(home, "missing workspace home button");
    assert.match(home[0], />cool-mono</);
    assert.doesNotMatch(home[0], /cool-mono\//);
  });

  await t.test("settings toggle reflects open state via aria-pressed", () => {
    const html = output({ settingsOpen: true });
    assert.match(buttonTag(html, "Workspace settings"), /aria-pressed="true"/);
  });

  await t.test("busy disables opening settings but not home; open settings can close", () => {
    const busyHtml = output({ busy: true });
    assert.match(buttonTag(busyHtml, "Workspace settings"), /disabled/);
    assert.match(busyHtml, /Wait for the current Bazel operation/);
    assert.doesNotMatch(buttonTag(busyHtml, "Workspace home"), /disabled/);

    const closing = output({ busy: true, settingsOpen: true });
    const gear = buttonTag(closing, "Workspace settings");
    assert.match(gear, /aria-pressed="true"/);
    assert.doesNotMatch(gear, /disabled/);
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
