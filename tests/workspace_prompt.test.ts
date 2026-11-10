import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { TargetScope } from "../src/lib/types.ts";

const SCOPE: TargetScope = { include: ["//..."] };

test("WorkspacePrompt supports direct path input and folder picking", async (t) => {
  const cacheDir = await mkdtemp(join(tmpdir(), "zeal-workspace-prompt-"));
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
    const { default: WorkspacePrompt } = await server.ssrLoadModule(
      "/src/lib/WorkspacePrompt.svelte",
    );
    const { default: CopyButton } = await server.ssrLoadModule("/src/lib/CopyButton.svelte");
    const { render } = await server.ssrLoadModule("svelte/server");

    function output(initial: string, include = SCOPE.include, bazelAvailable = true): string {
      return render(WorkspacePrompt, {
        props: { initial, initialScope: { include }, bazelAvailable },
      })
        .body.replace(/<!--[\s\S]*?-->/g, "")
        .replace(/\s+/g, " ");
    }

    function tag(html: string, pattern: RegExp): string {
      const match = html.match(pattern);
      assert.ok(match, `missing element matching ${pattern}`);
      return match[0];
    }

    function workspaceField(html: string): string {
      const label = tag(
        html,
        /<label\b[^>]*class="[^"]*workspace-selection[^"]*"[^>]*>[\s\S]*?<\/label>/,
      );
      return tag(label, /<input\b[^>]*>/);
    }

    function chooseButton(html: string): string {
      return tag(html, /<button[^>]*>[\s\S]*?Choose folder…[\s\S]*?<\/button>/);
    }

    function submitButton(html: string): string {
      return tag(html, /<button[^>]*type="submit"[^>]*>/);
    }

    await t.test("an empty workspace accepts a typed path and disables open", () => {
      const html = output("");
      const input = workspaceField(html);
      assert.doesNotMatch(input, /\bdisabled\b|\breadonly\b/);
      const choose = chooseButton(html);
      assert.match(choose, /type="button"/);
      assert.doesNotMatch(choose.slice(0, choose.indexOf(">") + 1), /\bdisabled\b/);
      assert.match(submitButton(html), /\bdisabled\b/);
      assert.match(html, /Do not open an untrusted repository\./);
      assert.match(html, /target patterns/);
      assert.match(html, /scope-patterns/);
    });

    await t.test("an initial workspace fills the path input and enables open", () => {
      const html = output("/workspace");
      const input = workspaceField(html);
      assert.match(input, /value="\/workspace"/);
      assert.doesNotMatch(input, /\bdisabled\b|\breadonly\b/);
      assert.doesNotMatch(submitButton(html), /\bdisabled\b/);
      assert.doesNotMatch(html, /restart Zeal/);
    });

    await t.test("a whitespace-only path disables open but stays editable", () => {
      const html = output("   ");
      const input = workspaceField(html);
      assert.match(input, /\bvalue=" +"/);
      assert.doesNotMatch(input, /\bdisabled\b|\breadonly\b/);
      assert.match(submitButton(html), /\bdisabled\b/);
    });

    await t.test(
      "an in-flight workspace open disables the path input, picker, and submit",
      async () => {
        const { app } = await server!.ssrLoadModule("/src/lib/state.svelte.ts");
        const opening = app.view.opening;
        Object.assign(app.view, { opening: true });
        try {
          const html = output("/workspace");
          const input = workspaceField(html);
          assert.match(input, /\bdisabled\b/);
          const choose = chooseButton(html);
          assert.match(choose.slice(0, choose.indexOf(">") + 1), /\bdisabled\b/);
          assert.match(submitButton(html), /\bdisabled\b/);
        } finally {
          Object.assign(app.view, { opening });
        }
      },
    );

    await t.test("CLI recording settings have a non-submitting copy button", () => {
      const html = render(WorkspacePrompt, {
        props: { initial: "", initialScope: SCOPE, bazelAvailable: true },
      }).body;
      assert.match(
        html,
        /<pre[^>]*><code[^>]*>build --bes_backend=grpc:\/\/127\.0\.0\.1:4718\nbuild --experimental_build_event_upload_strategy=local\nbuild --bes_timeout=5s\nbuild --experimental_collect_system_network_usage<\/code><\/pre>/,
      );
      const button = tag(html, /<button[^>]*aria-label="Copy \.bazelrc settings"[^>]*>/);
      assert.match(button, /type="button"/);
      assert.match(button, /title="Copy \.bazelrc settings"/);
      assert.doesNotMatch(button, /disabled/);
      assert.match(
        tag(html, /<button[^>]*aria-label="Copy \.bazelrc settings"[^>]*>[\s\S]*?<\/button>/),
        /<svg/,
      );
    });

    await t.test("copy button can be disabled", () => {
      const html = render(CopyButton, {
        props: {
          text: "build //...",
          label: "Copy settings",
          disabled: true,
          onerror: () => {},
        },
      }).body;
      assert.match(tag(html, /<button[^>]*>/), /disabled/);
    });

    await t.test("the default workspace-wide scope shows an advisory warning", () => {
      const html = output("/workspace");
      assert.match(html, /id="scope-warning"/);
      assert.match(html, /may be slow in large workspaces/);
      assert.match(html, /Consider using a narrower scope/);
      assert.match(tag(html, /<textarea\b[^>]*>/), /aria-describedby="scope-help scope-warning"/);
      assert.doesNotMatch(submitButton(html), /\bdisabled\b/);
    });

    for (const include of [[" \t//... \t"], ["//app/...", "", " //... ", "//tools:all"]]) {
      await t.test(
        `workspace-wide patterns warn after trimming: ${JSON.stringify(include)}`,
        () => {
          const html = output("/workspace", include);
          assert.match(html, /id="scope-warning"/);
          assert.match(
            tag(html, /<textarea\b[^>]*>/),
            /aria-describedby="scope-help scope-warning"/,
          );
          assert.doesNotMatch(submitButton(html), /\bdisabled\b/);
        },
      );
    }

    for (const include of [["//app/..."], ["//app:all"], ["", " \t", ""]]) {
      await t.test(`narrow or blank patterns do not warn: ${JSON.stringify(include)}`, () => {
        const html = output("/workspace", include);
        assert.doesNotMatch(html, /scope-warning/);
        assert.match(tag(html, /<textarea\b[^>]*>/), /aria-describedby="scope-help"/);
      });
    }

    await t.test("missing bazel keeps path input and picker usable", () => {
      const html = output("/workspace", SCOPE.include, false);
      assert.match(html, /Please install either tool and add it to your PATH, then restart Zeal/);
      const input = workspaceField(html);
      assert.doesNotMatch(input, /\bdisabled\b|\breadonly\b/);
      assert.match(input, /\bvalue="\/workspace"/);
      const choose = chooseButton(html);
      assert.doesNotMatch(choose.slice(0, choose.indexOf(">") + 1), /\bdisabled\b/);
      assert.match(submitButton(html), /\bdisabled\b/);
    });
  } finally {
    await server?.close();
    await rm(cacheDir, { recursive: true, force: true });
  }
});
