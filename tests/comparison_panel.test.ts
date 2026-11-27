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

  await t.test("enables one-click compare button when a previous successful build exists", () => {
    const html = output({
      left: detail({ id: "cur", cmd: "build", pattern: "//app:all", startedAt: 5000, exitCode: 1 }),
      history: [
        { id: "cur", cmd: "build", pattern: "//app:all", startedAt: 5000, exitCode: 1 },
        {
          id: "prev-ok",
          cmd: "build",
          pattern: "//app:all",
          startedAt: 3000,
          finishedAt: 4200,
          exitCode: 0,
        },
      ],
    });
    const button = html.match(/<button[^>]*compare-prev-btn[^>]*>/)?.[0];
    assert.ok(button, "previous successful button is missing");
    assert.doesNotMatch(button, /\bdisabled\b/);
    assert.ok(!html.includes("No previous successful build for this target pattern."));
    assert.match(html, /<select[^>]*aria-label="Recent invocations"/);
    assert.ok(html.includes('value="prev-ok"'));
  });

  await t.test("disables one-click compare button when no previous successful build exists", () => {
    const html = output({
      left: detail({ id: "cur", cmd: "build", pattern: "//app:all", startedAt: 5000, exitCode: 0 }),
      history: [{ id: "cur", cmd: "build", pattern: "//app:all", startedAt: 5000, exitCode: 0 }],
    });
    const button = html.match(/<button[^>]*compare-prev-btn[^>]*>/)?.[0];
    assert.ok(button, "previous successful button is missing");
    assert.match(button, /\bdisabled\b/);
    assert.ok(html.includes("No previous successful build for this target pattern."));
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
});
