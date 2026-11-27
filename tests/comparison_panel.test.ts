import { test } from "node:test";
import assert from "node:assert/strict";
import type { Bep, InvocationDetail, InvocationRecord, Section } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

function detail(
  record: Partial<InvocationRecord> = {},
  bep: Section<Bep> = { status: "ok", data: { targets: [], tests: [], truncated: false } },
): InvocationDetail {
  return {
    record: { id: "left-id", ...record },
    log: { status: "missing" },
    bep,
  };
}

test("ComparisonPanel renders the comparison form for the selected invocation", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: ComparisonPanel } = await load("/src/lib/invocation/ComparisonPanel.svelte");

  function output(overrides: Record<string, unknown> = {}): string {
    return render(ComparisonPanel, {
      props: {
        left: detail(),
        leftLoading: false,
        leftError: null,
        workspace: "/workspace",
        runningIds: [],
        ...overrides,
      },
    }).body.replace(/<!--[\s\S]*?-->/g, "");
  }

  function inputTag(html: string): string {
    const input = html.match(/<input[^>]*id="compare-invocation-id"[^>]*>/)?.[0];
    assert.ok(input, "comparison input is missing");
    return input;
  }

  await t.test("shows the selected ID with a labeled, enabled input", () => {
    const html = output();
    assert.ok(html.includes("left-id"));
    assert.match(html, /<table[^>]*aria-label="Invocation comparison"/);
    assert.ok(!html.includes("<caption>"));
    assert.match(
      html,
      /<label[^>]*for="compare-invocation-id"[^>]*>\s*Comparison invocation\s*<\/label>/,
    );
    assert.doesNotMatch(inputTag(html), /\bdisabled\b/);
    assert.match(inputTag(html), /placeholder="[^"]+"/);
  });

  await t.test("disables the input while the left detail is loading", () => {
    const html = output({ left: null, leftLoading: true });
    assert.match(inputTag(html), /\bdisabled\b/);
    assert.match(html, /Loading…/);
  });

  await t.test("disables the input when the left detail failed", () => {
    const html = output({ leftError: "read failed" });
    assert.match(inputTag(html), /\bdisabled\b/);
    assert.ok(html.includes("read failed"));
  });

  await t.test("disables the input while the compared invocation is running", () => {
    const html = output({ runningIds: ["left-id"] });
    assert.match(inputTag(html), /\bdisabled\b/);
    assert.ok(html.includes("Comparison is available"));
  });

  await t.test("keeps the input enabled when a different invocation is running", () => {
    const html = output({ runningIds: ["other-id"] });
    assert.doesNotMatch(inputTag(html), /\bdisabled\b/);
  });

  await t.test("escapes option values in the rendered HTML", () => {
    const html = output({
      left: detail(
        {},
        {
          status: "ok",
          data: {
            targets: [],
            tests: [],
            truncated: false,
            options: {
              startupOptions: [],
              explicitStartupOptions: [],
              cmdLine: ["<script>alert(1)</script>"],
              explicitCmdLine: [],
            },
          },
        },
      ),
    });
    assert.doesNotMatch(html, /<script>alert/);
    assert.match(html, /&lt;script>alert\(1\)&lt;\/script>/);
  });

  await t.test("surfaces a non-ok BEP section as a notice", () => {
    const html = output({
      left: detail(
        {},
        {
          status: "incomplete",
          data: { targets: [], tests: [], truncated: false },
          note: "saved prefix missing",
        },
      ),
    });
    assert.ok(html.includes("saved prefix missing"));
  });

  await t.test("InvocationView shows the Compare tab without the form on All", async () => {
    const { emptyView } = await load("/src/lib/session.ts");
    const { default: InvocationView } = await load("/src/lib/InvocationView.svelte");
    const left = detail();
    const view = {
      ...emptyView(),
      content: { kind: "invocation" as const, id: "left-id", page: "targets" as const },
      invocations: [left.record],
      invocation: { record: left.record },
      detail: left,
    };
    const html = render(InvocationView, {
      props: {
        view,
        reload: () => {},
        reloadProfile: () => {},
        onstop: () => {},
        onconsole: () => {},
      },
    }).body.replace(/<!--[\s\S]*?-->/g, "");
    assert.match(html, /id="invocation-tab-Compare"/);
    assert.ok(!html.includes("compare-invocation-id"));
    for (const section of ["Targets", "Details", "Logs", "Timing"]) {
      assert.ok(
        html.includes(`id="invocation-heading-${section}"`),
        `missing section heading ${section}`,
      );
    }
    assert.match(
      html,
      /<div(?=[^>]*id="invocation-panel-All")(?=[^>]*aria-labelledby="invocation-tab-All")[^>]*>/,
    );
  });

  await t.test("InvocationHeader leads the h1 with the command", async () => {
    const { default: InvocationHeader } = await load("/src/lib/InvocationHeader.svelte");
    const { invocationStatus } = await load("/src/lib/invocation.ts");

    function header(record: InvocationRecord): string {
      return render(InvocationHeader, {
        props: { record, status: invocationStatus(record, null), onstop: () => {} },
      }).body.replace(/<!--[\s\S]*?-->/g, "");
    }

    function h1Text(html: string): string {
      const match = html.match(/<h1[^>]*>([\s\S]*?)<\/h1>/);
      assert.ok(match, "header h1 is missing");
      return match[1]
        .replace(/<[^>]*>/g, "")
        .replace(/\s+/g, " ")
        .trim();
    }

    const withCmd = header({ id: "run-id", cmd: "build", pattern: "//app:server" });
    assert.equal(h1Text(withCmd), "build //app:server");
    assert.ok(!withCmd.includes('class="badge"'));

    assert.equal(h1Text(header({ id: "run-id", pattern: "//app:server" })), "//app:server");
    assert.equal(h1Text(header({ id: "run-id", cmd: "build" })), "build run-id");
  });

  await t.test(
    "InvocationView disables Re-run while detail is loading and enables it once loaded",
    async () => {
      const { emptyView } = await load("/src/lib/session.ts");
      const { default: InvocationView } = await load("/src/lib/InvocationView.svelte");
      const left = detail({ cmd: "build", pattern: "//app:server", workspace: "/workspace" });
      const base = {
        ...emptyView(),
        core: {
          revision: 1,
          besError: null,
          besEndpoint: "grpc://127.0.0.1:4718",
          bazel: "/usr/bin/bazel",
          lastWorkspace: left.record.workspace!,
          workspaces: [{ workspace: left.record.workspace!, scope: { include: ["//..."] } }],
          invocations: [],
        },
        state: { workspace: left.record.workspace!, scope: { include: ["//..."] } },
        invocations: [left.record],
        invocation: { record: left.record },
      };

      const loadingHtml = render(InvocationView, {
        props: {
          view: { ...base, detail: null, detailLoading: true },
          reload: () => {},
          reloadProfile: () => {},
          onstop: () => {},
          onrerun: () => {},
        },
      }).body;
      assert.match(loadingHtml, /<button[^>]*disabled[^>]*>\s*Re-run\s*<\/button>/);

      const readyHtml = render(InvocationView, {
        props: {
          view: { ...base, detail: left, detailLoading: false },
          reload: () => {},
          reloadProfile: () => {},
          onstop: () => {},
          onrerun: () => {},
        },
      }).body;
      assert.match(readyHtml, /<button(?![^>]*disabled)[^>]*>\s*Re-run\s*<\/button>/);
    },
  );
});
