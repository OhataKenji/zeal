import { test } from "node:test";
import assert from "node:assert/strict";
import { setImmediate } from "node:timers/promises";
import { configActions } from "../src/lib/workspace-configs.ts";
import { createSession, type Transport } from "../src/lib/session.ts";
import type { AppState, BazelConfig, WorkspaceInfo } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

function config(name: string, commands: string[]): BazelConfig {
  return {
    name,
    entries: commands.map((command, i) => ({
      command,
      flags: `--flag${i}`,
      source: ".bazelrc",
      line: i + 1,
    })),
  };
}

test("configActions follows Bazel command inheritance", async (t) => {
  const cases: Array<[string, string[], boolean, string | null]> = [
    ["always:ci", ["always"], true, "build"],
    ["build:ci", ["build"], true, "build"],
    ["test:ci", ["test"], true, "test"],
    ["run:local", ["run"], false, "run"],
    ["query:inspect", ["query"], false, null],
    ["common:netlocal", ["common"], true, "build"],
    ["cquery:transitions", ["cquery"], true, null],
    ["aquery:brief", ["aquery"], false, null],
    // `always`/`common`/`build` win the run command over `test`/`run`.
    ["mixed:ci", ["run", "test", "common"], true, "build"],
    ["test-only", ["test"], true, "test"],
  ];
  for (const [name, commands, canCquery, runCmd] of cases) {
    await t.test(name, () => {
      assert.deepEqual(configActions(config(name.split(":")[0], commands)), {
        canCquery,
        runCmd,
      });
    });
  }
});

const INFO: WorkspaceInfo = {
  workspace: "/ws",
  release: "release 8.7.0",
  outputBase: "/ob",
  executionRoot: "/ob/execroot/_main",
  bazelBin: "/ob/execroot/_main/bazel-out/bin",
  bazelTestlogs: "/ob/execroot/_main/bazel-out/testlogs",
  repositoryCache: "/cache",
  rcAnnouncements: [
    { source: "/etc/bazel.bazelrc", scope: "common", flags: "--jobs=8" },
    { source: "/ws/.bazelrc", scope: "config:ci", flags: "--compilation_mode=opt" },
  ],
  configs: [config("ci", ["common", "build", "test"]), config("inspect", ["query"])],
  infoError: null,
};

test("WorkspaceView shows Overview by default and keeps Configs behind its tab", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: WorkspaceView } = await load("/src/lib/WorkspaceView.svelte");

  function output(
    info: WorkspaceInfo | null,
    initialTab: "overview" | "configs" = "overview",
  ): string {
    return render(WorkspaceView, {
      props: {
        workspace: "/ws",
        scope: { include: ["//..."] },
        info,
        loading: false,
        error: null,
        onretry: () => {},
        onsavescope: async () => true,
        onrunconfig: () => {},
        oncqueryconfig: () => {},
        initialTab,
      },
    })
      .body.replace(/<!--[\s\S]*?-->/g, "")
      .replace(/\s+/g, " ");
  }

  await t.test("overview tab is selected and config content stays hidden", () => {
    const html = output(INFO);
    assert.equal((html.match(/role="tab"/g) ?? []).length, 2);
    assert.match(html, /aria-selected="true"[^>]*>Overview</);
    assert.match(html, /aria-selected="false"[^>]*>Configs</);
    assert.match(html, /Directories &amp; Caches \(bazel info\)/);
    assert.match(html, /Target Discovery Scope/);
    assert.match(html, /Local BES Receiver/);
    assert.doesNotMatch(html, /Config Definitions/);
    assert.doesNotMatch(html, /Active RC Provenance/);
    assert.doesNotMatch(html, /--config=ci/);
  });

  await t.test("infoError surfaces on the overview directories section", () => {
    const html = output({ ...INFO, infoError: "no bazel" });
    assert.match(html, /bazel info failed: no bazel/);
  });

  await t.test("configs tab lists definitions, provenance, and gated actions", () => {
    const html = output(INFO, "configs");
    assert.match(html, /aria-selected="false"[^>]*>Overview</);
    assert.match(html, /aria-selected="true"[^>]*>Configs</);
    assert.match(html, /Config Definitions \(\.bazelrc\)/);
    assert.match(html, /Active RC Provenance \(bazel info --announce_rc\)/);
    // `ci` inherits common+build+test: both launch actions are offered.
    assert.match(html, /--config=ci/);
    // `inspect` is query-only: listed but without run/cquery affordances.
    assert.match(html, /--config=inspect/);
    const ciBlock = html.slice(html.indexOf("--config=ci"), html.indexOf("--config=inspect"));
    assert.equal((ciBlock.match(/Run with config/g) ?? []).length, 1);
    assert.equal((ciBlock.match(/Open in cquery/g) ?? []).length, 1);
    const inspectBlock = html.slice(html.indexOf("--config=inspect"));
    assert.doesNotMatch(inspectBlock, /Run with config|Open in cquery/);
    // Entries keep their rc source/line provenance.
    assert.match(html, /\.bazelrc:1/);
    // announce_rc rows render source, scope, and flags.
    assert.match(html, /\/etc\/bazel\.bazelrc/);
    assert.match(html, /config:ci/);
    assert.match(html, /not the expanded result/);
  });

  await t.test("configs tab surfaces infoError from bazel info", () => {
    const html = output({ ...INFO, infoError: "no bazel" }, "configs");
    assert.match(html, /bazel info --announce_rc failed: no bazel/);
  });
});

test("workspace info loads even when Bazel is unavailable", async () => {
  const WS = "/ws";
  const core: AppState = {
    revision: 1,
    besError: null,
    besReady: true,
    bazel: null,
    lastWorkspace: WS,
    workspaces: [{ workspace: WS, scope: { include: ["//..."] } }],
    invocations: [],
  };
  const invoked: string[] = [];
  const transport: Transport = {
    invoke: <T>(command: string) => {
      if (command === "get_state") return Promise.resolve(core) as Promise<T>;
      invoked.push(command);
      return new Promise<T>(() => {});
    },
    listen: async () => () => {},
  };
  const session = createSession(transport, () => {}, "workspace");
  try {
    await session.init();
    await setImmediate();
    assert.ok(invoked.includes("get_workspace_info"));
  } finally {
    session.dispose();
  }
});
