import { test } from "node:test";
import assert from "node:assert/strict";
import { rerunArgs, summarizeRunners } from "../src/lib/invocation.ts";

test("summarizeRunners excludes total row and computes cache hit percentage", () => {
  const sum = summarizeRunners([
    { name: "total", count: 42 },
    { name: "disk cache hit", count: 28 },
    { name: "remote cache hit", count: 6 },
    { name: "darwin-sandbox", count: 8 },
  ]);
  assert.deepEqual(sum, {
    total: 42,
    cached: 34,
    hitPct: 81,
    runners: [
      { name: "disk cache hit", count: 28, cached: true },
      { name: "remote cache hit", count: 6, cached: true },
      { name: "darwin-sandbox", count: 8, cached: false },
    ],
  });
  assert.equal(summarizeRunners([]), null);
  assert.equal(summarizeRunners([{ name: "total", count: 5 }]), null);
  assert.deepEqual(
    summarizeRunners([
      { name: "total", count: 10 },
      { name: "disk cache hit", count: 9 },
      { name: "internal", count: 1 },
    ]),
    {
      total: 9,
      cached: 9,
      hitPct: 100,
      runners: [{ name: "disk cache hit", count: 9, cached: true }],
    },
  );
});

test("rerunArgs allows build/test/run with valid patterns and rejects invalid ones", () => {
  assert.deepEqual(rerunArgs({ id: "1", cmd: "test", pattern: "//app:all" }), {
    cmd: "test",
    pattern: "//app:all",
    flags: "",
  });
  assert.equal(rerunArgs({ id: "1", cmd: "query", pattern: "//..." }), null);
  assert.equal(rerunArgs({ id: "1", cmd: "build", pattern: "-//app:bad" }), null);
  assert.equal(rerunArgs({ id: "1", cmd: "build" }), null);
});

test("rerunArgs preserves user flags while stripping Zelith defaults and BES identity options", () => {
  const record = { id: "1", cmd: "build", pattern: "//:mode_sensitive" };
  const bep = (explicitCmdLine: string[]) => ({
    targets: [],
    tests: [],
    truncated: false,
    options: {
      startupOptions: [],
      explicitStartupOptions: [],
      cmdLine: explicitCmdLine,
      explicitCmdLine,
    },
  });

  // Zelith-launched build: strip the 7 injected defaults, keep user flags and overrides.
  assert.deepEqual(
    rerunArgs(
      record,
      bep([
        "--bes_backend=grpc://127.0.0.1:4718",
        "--bes_proxy=",
        "--bes_upload_mode=wait_for_upload_complete",
        "--tool_tag=zelith",
        "--color=yes",
        "--curses=no",
        "--invocation_id=1aa7f527-6489-4848-bc6f-5e8ca927f812",
        "--define=qa_mode=enabled",
        "--color=no",
        "--define=msg=hello world",
      ]),
    ),
    {
      cmd: "build",
      pattern: "//:mode_sensitive",
      flags: "--define=qa_mode=enabled --color=no --define='msg=hello world'",
    },
  );

  // Zelith-launched build with no user flags produces an empty flags string.
  assert.deepEqual(
    rerunArgs(
      record,
      bep([
        "--bes_backend=grpc://127.0.0.1:50000",
        "--bes_proxy=",
        "--bes_upload_mode=wait_for_upload_complete",
        "--tool_tag=zelith",
        "--color=yes",
        "--curses=no",
        "--invocation_id=abc",
      ]),
    ),
    {
      cmd: "build",
      pattern: "//:mode_sensitive",
      flags: "",
    },
  );

  // External/terminal build: strip --bes_backend and --invocation_id, keep all other user flags.
  assert.deepEqual(
    rerunArgs(
      record,
      bep([
        "--bes_backend=grpc://127.0.0.1:4718",
        "--config=opt",
        "--invocation_id",
        "external-id",
        "--define=qa_mode=enabled",
        "--bes_backend",
        "grpc://127.0.0.1:9999",
      ]),
    ),
    {
      cmd: "build",
      pattern: "//:mode_sensitive",
      flags: "--config=opt --define=qa_mode=enabled",
    },
  );
});
