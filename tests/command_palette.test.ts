import { test } from "node:test";
import assert from "node:assert/strict";
import { createSession, emptyView } from "../src/lib/session.ts";
import type { AppState, WorkspaceState } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

const WS: WorkspaceState = {
  workspace: "/workspace",
  scope: { include: ["//app/..."] },
};

const CORE: AppState = {
  revision: 1,
  besError: null,
  besEndpoint: "grpc://127.0.0.1:4718",
  bazel: "/bin/bazel",
  lastWorkspace: "/workspace",
  workspaces: [WS],
  invocations: [],
};

const SNAPSHOT = {
  meta: { fetchedAt: 1, status: "ok" as const },
  targets: [
    {
      label: "//app:server",
      pkg: "app",
      name: "server",
      ruleClass: "java_binary",
      location: null,
    },
  ],
};

function renderSession() {
  return createSession(
    {
      invoke: async () => {
        throw new Error("render tests do not call the core");
      },
      listen: async () => () => {},
    },
    () => {},
  );
}

test("AppView shows the palette trigger only with a workspace", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: AppView } = await load("/src/AppView.svelte");
  const session = renderSession();

  const view = Object.assign(emptyView("targets"), {
    core: CORE,
    state: WS,
    snapshot: SNAPSHOT,
    invocations: [],
    invocationRows: [],
  });
  const html = render(AppView, { props: { view, session } }).body;
  assert.match(html, /class="toolbar-search"/);
  assert.match(html, /Search targets, invocations, commands…/);
  assert.match(html, /Ctrl\+K|⌘K/);

  const empty = render(AppView, { props: { view: emptyView("targets"), session } }).body;
  assert.doesNotMatch(empty, /toolbar-search/);
});

test("CommandPalette renders an accessible combobox listbox with grouped options", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: CommandPalette } = await load("/src/lib/CommandPalette.svelte");

  const invocations = [
    {
      record: {
        id: "run-1",
        cmd: "build",
        pattern: "//app:server",
        startedAt: 1,
        finishedAt: 2,
        exitCode: 0,
      },
    },
  ];
  const html = render(CommandPalette, {
    props: {
      open: true,
      targetIndex: null,
      invocations,
      canRun: true,
      canRefresh: true,
      isMac: false,
      onselect: () => {},
      onclose: () => {},
    },
  }).body;

  assert.match(html, /role="combobox"/);
  assert.match(html, /aria-expanded="true"/);
  assert.match(html, /aria-autocomplete="list"/);
  assert.match(html, /aria-activedescendant="[^"]+-opt-0"/);
  assert.match(html, /maxlength="512"/);
  assert.match(html, /role="listbox"/);
  assert.match(html, /role="group"/);
  assert.match(html, />Commands</);
  assert.match(html, />Invocations</);
  assert.match(html, />Go to Targets</);
  assert.match(html, />Run Bazel…</);
  assert.match(html, />Ctrl\+Shift\+R</);
  const options = html.match(/role="option"/g) ?? [];
  // Nine commands plus the single recent invocation.
  assert.equal(options.length, 10);
  assert.match(html, /aria-selected="true"/);
});

test("a closed CommandPalette renders no options", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: CommandPalette } = await load("/src/lib/CommandPalette.svelte");
  const html = render(CommandPalette, {
    props: {
      open: false,
      targetIndex: null,
      invocations: [
        {
          record: {
            id: "run-1",
            cmd: "build",
            pattern: "//app:server",
            startedAt: 1,
            finishedAt: 2,
            exitCode: 0,
          },
        },
      ],
      canRun: true,
      canRefresh: true,
      isMac: false,
      onselect: () => {},
      onclose: () => {},
    },
  }).body;
  assert.match(html, /aria-expanded="false"/);
  assert.doesNotMatch(html, /role="option"/);
  assert.doesNotMatch(html, /role="group"/);
});
