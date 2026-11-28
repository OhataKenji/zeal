import { test } from "node:test";
import assert from "node:assert/strict";
import type { InvocationDetail, InvocationRecord } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

function makeDetail(explicitCmdLine: string[]): InvocationDetail {
  const record: InvocationRecord = {
    id: "inv-1",
    workspace: "/ws",
    cmd: "test",
    pattern: "//pkg:all",
    startedAt: 1,
    finishedAt: 2,
    exitCode: 3,
  };
  return {
    record,
    log: { status: "ok", data: "" },
    bep: {
      status: "ok",
      data: {
        targets: [],
        tests: [
          { label: "//pkg:ok_test", status: "PASSED" },
          { label: "//pkg:broken_test", status: "FAILED" },
        ],
        options: {
          startupOptions: [],
          explicitStartupOptions: [],
          cmdLine: explicitCmdLine,
          explicitCmdLine,
        },
        truncated: false,
      },
    },
  };
}

test("InvocationView and InvocationHeader carry explicitCmdLine into Re-run controls", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { emptyView } = await load("/src/lib/session.ts");
  const { default: InvocationView } = await load("/src/lib/InvocationView.svelte");

  function renderView(detail: InvocationDetail, detailLoading = false): string {
    const base = emptyView();
    const view = {
      ...base,
      core: {
        bazel: "/usr/bin/bazel",
        besEndpoint: "grpc://127.0.0.1:4718",
        workspaces: [{ workspace: "/ws", scope: { kind: "all" as const } }],
        invocations: [],
        diagnostics: [],
        revision: 1,
      },
      state: { workspace: "/ws", scope: { kind: "all" as const } },
      page: "history" as const,
      selectedInvocationId: detail.record.id,
      invocations: [detail.record],
      invocationRows: [{ record: detail.record }],
      invocation: { record: detail.record },
      detail,
      detailLoading,
    };
    return render(InvocationView, {
      props: {
        view,
        reload: () => {},
        reloadProfile: () => {},
        onstop: () => {},
        onrerun: () => {},
        onopenflags: () => {},
      },
    }).body.replace(/<!--[\s\S]*?-->/g, "");
  }

  await t.test("header Re-run and failed-test Re-run inherit filtered explicitCmdLine", () => {
    const detail = makeDetail([
      "--bes_backend=grpc://127.0.0.1:4718",
      "--bes_proxy=",
      "--bes_upload_mode=wait_for_upload_complete",
      "--invocation_id=inv-1",
      "--tool_tag=zelith",
      "--color=yes",
      "--curses=no",
      "--config=asan",
      "--test_env=FOO=bar baz",
    ]);
    const html = renderView(detail);
    assert.match(
      html,
      /<button(?=[^>]*title="Re-run: bazel test \/\/pkg:all --config=asan --test_env='FOO=bar baz'")(?![^>]*disabled)[^>]*>\s*Re-run\s*<\/button>/,
    );
    assert.match(html, /Re-run with flags…/);
    assert.match(
      html,
      /<button(?=[^>]*aria-label="Re-run \/\/pkg:broken_test")(?=[^>]*title="Re-run: bazel test \/\/pkg:broken_test --config=asan --test_env='FOO=bar baz'")(?![^>]*disabled)[^>]*>\s*Re-run\s*<\/button>/,
    );
    assert.doesNotMatch(html, /aria-label="Re-run \/\/pkg:ok_test"/);
  });

  await t.test("Re-run controls stay disabled while invocation detail is loading", () => {
    const detail = makeDetail(["--config=asan"]);
    const html = renderView(detail, true);
    assert.match(
      html,
      /<button(?=[^>]*class="button button-primary")(?=[^>]*disabled)(?![^>]*title=)[^>]*>\s*Re-run\s*<\/button>/,
    );
    assert.doesNotMatch(html, /Re-run with flags…/);
  });
});
