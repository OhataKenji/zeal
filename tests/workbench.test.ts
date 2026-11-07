import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { AppState, InvocationRecord } from "../src/lib/types.ts";

const IDLE: AppState = {
  besReady: false,
  scope: { include: ["//app/..."] },
  workspace: "/workspace",
  bazel: "/bin/bazel",
  ingest: "idle",
  snapshotFetchedAt: null,
  lastError: null,
  activeInvocation: null,
  stopping: false,
  invocationNote: null,
  query: "idle",
  detailQuery: "idle",
};

const RECORD: InvocationRecord = {
  id: "run-a",
  workspace: "/workspace",
  cmd: "build",
  pattern: "//:app",
  startedAt: 1,
};

const OTHER: InvocationRecord = {
  id: "run-b",
  workspace: "/workspace",
  cmd: "test",
  pattern: "//:other",
  finishedAt: 2,
  exitCode: 0,
};

test("workbench components render from props", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-workbench-"));
  let server: ViteDevServer | undefined;
  try {
    server = await createServer({
      root: fileURLToPath(new URL("..", import.meta.url)),
      configFile: false,
      cacheDir,
      plugins: [svelte()],
      appType: "custom",
      logLevel: "silent",
      server: { middlewareMode: true, watch: null, ws: false },
      optimizeDeps: { noDiscovery: true, include: [] },
      ssr: { optimizeDeps: { exclude: ["svelte"] } },
    });
    const { default: CommandComposer } = await server.ssrLoadModule(
      "/src/lib/workbench/CommandComposer.svelte",
    );
    const { default: ExplorerSidebar } = await server.ssrLoadModule(
      "/src/lib/workbench/ExplorerSidebar.svelte",
    );
    const { render } = await server.ssrLoadModule("svelte/server");
    const noop = () => {};

    function composerHtml(props: Record<string, unknown>): string {
      return render(CommandComposer, {
        props: {
          index: null,
          value: "",
          inputError: null,
          running: false,
          disabled: false,
          oninput: noop,
          onpick: noop,
          onstart: noop,
          onstop: noop,
          ...props,
        },
      }).body;
    }

    function sidebarHtml(props: Record<string, unknown>): string {
      return render(ExplorerSidebar, {
        props: {
          visible: true,
          state: IDLE,
          targets: {
            snapshot: null,
            loading: false,
            error: null,
            selection: null,
          },
          history: {
            records: [RECORD],
            error: null,
            selectedId: null,
            consoleId: null,
          },
          page: "explorer",
          revealRequest: 0,
          onselect: noop,
          oninvocation: noop,
          onpage: noop,
          onretryTargets: noop,
          onretryInvocations: noop,
          onTargetVisibility: noop,
          ...props,
        },
      }).body;
    }

    await t.test("composer defaults to an enabled Build action", () => {
      const html = composerHtml({});
      assert.match(html, /aria-label="Build"/);
      assert.doesNotMatch(html, /composer-invalid|disabled/);
      assert.doesNotMatch(html, /id="command-arguments-popover"/);
    });

    await t.test("composer marks the target input invalid and links the error", () => {
      const html = composerHtml({ inputError: "invalid target" });
      assert.match(html, /composer-invalid/);
      assert.match(html, /aria-invalid="true"/);
      assert.match(html, /aria-describedby="composer-target-error"/);
    });

    await t.test("composer shows Stop and honors the disabled prop while running", () => {
      const html = composerHtml({ running: true, disabled: true });
      assert.match(html, /aria-label="Stop"[^>]*disabled/);
    });

    await t.test("sidebar renders both panes expanded with history rows", () => {
      const html = sidebarHtml({});
      assert.match(html, /class="sidebar"/);
      assert.match(html, /Targets/);
      assert.match(html, /Invocations/);
      assert.match(html, /aria-expanded="true"/);
      assert.match(html, /class="invocation-command">build</);
      assert.match(html, /class="invocation-pattern">\/\/:app</);
    });

    await t.test("sidebar marks the selected invocation row", () => {
      const html = sidebarHtml({
        history: {
          records: [RECORD, OTHER],
          error: null,
          selectedId: "run-a",
          consoleId: null,
        },
      });
      const rows =
        html.match(/<button[^>]*class="invocation-row[^"]*"[^>]*aria-current="true"[^>]*>/g) ?? [];
      assert.equal(rows.length, 1);
      assert.match(rows[0], /aria-label="build \/\/:app/);
    });

    await t.test("sidebar marks the open console record active too", () => {
      const html = sidebarHtml({
        history: {
          records: [RECORD, OTHER],
          error: null,
          selectedId: null,
          consoleId: "run-b",
        },
      });
      assert.match(html, /aria-label="test \/\/:other[^"]*"[^>]*aria-current="true"/);
    });

    await t.test("sidebar highlights only the canonical page kind", () => {
      const analytics = sidebarHtml({ page: "analytics" });
      assert.match(analytics, /analytics-link active/);
      const invocation = sidebarHtml({
        page: "invocation",
        history: {
          records: [RECORD],
          error: null,
          selectedId: "run-a",
          consoleId: null,
        },
      });
      assert.doesNotMatch(invocation, /analytics-link active/);
    });

    await t.test("sidebar renders no aside while hidden", () => {
      const html = sidebarHtml({ visible: false });
      assert.doesNotMatch(html, /class="sidebar"/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
