import { test } from "node:test";
import assert from "node:assert/strict";
import {
  comparisonRows,
  filterBaselineOptions,
  formatInvocationOption,
  normalizePattern,
  previousSuccessfulInvocation,
  recentComparableInvocations,
} from "../src/lib/invocation-diff.ts";
import type { ComparisonInvocation, ComparisonPart } from "../src/lib/invocation-diff.ts";
import type { Bep, InvocationRecord, Section } from "../src/lib/types.ts";

function invocation(
  record: Partial<InvocationRecord> = {},
  bep: Section<Bep> = { status: "missing" },
): ComparisonInvocation {
  return { record: { id: "invocation", ...record }, bep };
}

function options(overrides: Partial<NonNullable<Bep["options"]>> = {}) {
  const merged: NonNullable<Bep["options"]> = {
    startupOptions: [],
    explicitStartupOptions: [],
    cmdLine: [],
    explicitCmdLine: [],
    ...overrides,
  };
  return {
    status: "ok" as const,
    data: { targets: [], tests: [], truncated: false, options: merged, runnerCounts: [] },
  };
}

function withRunners(cached: number, exec: number): Section<Bep> {
  return {
    status: "ok",
    data: {
      targets: [],
      tests: [],
      truncated: false,
      runnerCounts: [
        ...(cached > 0 ? [{ name: "disk cache hit", count: cached }] : []),
        ...(exec > 0 ? [{ name: "linux-sandbox", count: exec }] : []),
      ],
    },
  };
}

function row(rows: ReturnType<typeof comparisonRows>, label: string) {
  const found = rows.find((candidate) => candidate.label === label);
  assert.ok(found, `missing row ${label}`);
  return found;
}

function text(parts: ComparisonPart[]): string {
  return parts.map((part) => part.text).join("");
}

function changedText(parts: ComparisonPart[]): string {
  return text(parts.filter((part) => part.changed));
}

test("maps each facet to its record or BEP source in diagnostic-first order", () => {
  const detail = invocation(
    {
      id: "run",
      cmd: "build",
      pattern: "//app:all",
      workspace: "/repo/wt",
      exitCode: 0,
      startedAt: 1000,
      finishedAt: 2500,
    },
    options({
      cmdLine: ["--cmd-full"],
      explicitCmdLine: ["--cmd-explicit"],
      startupOptions: ["--startup-full"],
      explicitStartupOptions: ["--startup-explicit"],
    }),
  );
  const rows = comparisonRows(detail, structuredClone(detail));
  assert.deepEqual(
    rows.map((candidate) => [candidate.label, candidate.left, candidate.right]),
    [
      ["Duration", "1.500s", "1.500s"],
      ["Spawn cache hits", "0 spawns", "0 spawns"],
      ["Exit code", "0", "0"],
      ["Command", "build", "build"],
      ["Pattern", "//app:all", "//app:all"],
      ["Workspace", "/repo/wt", "/repo/wt"],
      ["Explicit command line", "--cmd-explicit", "--cmd-explicit"],
      ["Full command line", "--cmd-full", "--cmd-full"],
      ["Explicit startup options", "--startup-explicit", "--startup-explicit"],
      ["Full startup options", "--startup-full", "--startup-full"],
      ["Start date", new Date(1000).toLocaleString(), new Date(1000).toLocaleString()],
      ["Invocation ID", "run", "run"],
    ],
  );
  assert.ok(rows.every((candidate) => candidate.status === "same"));
  assert.ok(
    rows.every(
      (candidate) =>
        candidate.leftParts.every((part) => !part.changed) &&
        candidate.rightParts.every((part) => !part.changed),
    ),
  );
});

test("incomplete BEP data leaves option and spawn cache rows unknown", () => {
  const rows = comparisonRows(
    invocation({ id: "x", cmd: "build" }, options({ cmdLine: ["--known"] })),
    invocation(
      { id: "x", cmd: "test" },
      {
        status: "incomplete",
        data: options({ cmdLine: ["--different"] }).data,
        note: "saved prefix missing",
      },
    ),
  );
  assert.equal(row(rows, "Command").status, "changed");
  const full = row(rows, "Full command line");
  assert.equal(full.status, "unknown");
  assert.equal(full.left, "--known");
  assert.equal(full.right, "Not recorded");
  const cache = row(rows, "Spawn cache hits");
  assert.equal(cache.status, "unknown");
  assert.equal(cache.right, "Not recorded");
});

test("option arrays preserve order, duplicates, and empty vs absent distinction", () => {
  const order = comparisonRows(
    invocation({ id: "x" }, options({ cmdLine: ["a", "b", "a"] })),
    invocation({ id: "x" }, options({ cmdLine: ["b", "a", "a"] })),
  );
  assert.equal(row(order, "Full command line").status, "changed");
  assert.equal(row(order, "Full command line").left, "a\nb\na");
  assert.equal(row(order, "Full command line").right, "b\na\na");

  const emptyVsAbsent = comparisonRows(
    invocation({ id: "x" }, options({ cmdLine: [] })),
    invocation({ id: "x" }, { status: "ok", data: { targets: [], tests: [], truncated: false } }),
  );
  const full = row(emptyVsAbsent, "Full command line");
  assert.equal(full.left, "No options");
  assert.equal(full.right, "Not recorded");
  assert.equal(full.status, "unknown");

  const emptyVsSet = comparisonRows(
    invocation({ id: "x" }, options({ cmdLine: [] })),
    invocation({ id: "x" }, options({ cmdLine: ["--only"] })),
  );
  assert.deepEqual(row(emptyVsSet, "Full command line").leftParts, [
    { text: "No options", changed: false },
  ]);
  assert.deepEqual(row(emptyVsSet, "Full command line").rightParts, [
    { text: "--only", changed: true },
  ]);
});

test("duration and exit code rows format inline deltas", () => {
  const slower = row(
    comparisonRows(
      invocation({ id: "base", exitCode: 0, startedAt: 1000, finishedAt: 3500 }),
      invocation({ id: "cur", exitCode: 0, startedAt: 1000, finishedAt: 6000 }),
    ),
    "Duration",
  );
  assert.equal(slower.left, "2.500s");
  assert.equal(slower.right, "5.000s (+2.500s)");
  assert.ok(
    slower.leftParts.every((p) => !p.changed) && slower.rightParts.every((p) => !p.changed),
  );

  const earlyFail = comparisonRows(
    invocation({ id: "base", exitCode: 0, startedAt: 1000, finishedAt: 16000 }),
    invocation({ id: "cur", exitCode: 1, startedAt: 1000, finishedAt: 1500 }),
  );
  assert.equal(row(earlyFail, "Duration").right, "0.500s (-14.500s)");
  assert.equal(row(earlyFail, "Exit code").right, "1 (Failed)");

  const fixed = comparisonRows(
    invocation({ id: "base", exitCode: 1, startedAt: 1000, finishedAt: 2000 }),
    invocation({ id: "cur", exitCode: 0, startedAt: 1000, finishedAt: 5000 }),
  );
  assert.equal(row(fixed, "Exit code").right, "0 (Fixed)");

  const interrupted = comparisonRows(
    invocation({ id: "base", exitCode: 0, startedAt: 1000, finishedAt: 2000 }),
    invocation({ id: "cur", exitCode: 8, startedAt: 1000, finishedAt: 2500 }),
  );
  assert.equal(row(interrupted, "Exit code").right, "8 (Interrupted)");
  assert.equal(
    row(
      comparisonRows(
        invocation({ id: "base", exitCode: 8 }),
        invocation({ id: "cur", exitCode: 0 }),
      ),
      "Exit code",
    ).right,
    "0",
  );

  const same = row(
    comparisonRows(
      invocation({ id: "base", exitCode: 0, startedAt: 5000, finishedAt: 6000 }),
      invocation({ id: "cur", exitCode: 0, startedAt: 1000, finishedAt: 2000 }),
    ),
    "Duration",
  );
  assert.equal(same.right, "1.000s");
  assert.equal(same.status, "same");

  const missing = row(
    comparisonRows(
      invocation({ id: "base", startedAt: 1000 }),
      invocation({ id: "cur", startedAt: 2500, finishedAt: 1000 }),
    ),
    "Duration",
  );
  assert.equal(missing.status, "unknown");
});

test("spawn cache hits handles equal totals, changed totals, 0-spawn, and absent runnerCounts", () => {
  const drop = row(
    comparisonRows(
      invocation({ id: "base" }, withRunners(9, 1)),
      invocation({ id: "cur" }, withRunners(2, 8)),
    ),
    "Spawn cache hits",
  );
  assert.equal(drop.left, "90% (9/10)");
  assert.equal(drop.right, "20% (2/10) (-70 pp)");
  assert.ok(drop.leftParts.every((p) => !p.changed) && drop.rightParts.every((p) => !p.changed));

  const incremental = row(
    comparisonRows(
      invocation({ id: "base" }, withRunners(95, 5)),
      invocation({ id: "cur" }, withRunners(0, 2)),
    ),
    "Spawn cache hits",
  );
  assert.equal(incremental.right, "0% (0/2) (-95 pp, -98 spawns)");

  const noop = row(
    comparisonRows(
      invocation({ id: "base" }, withRunners(9, 1)),
      invocation({ id: "cur" }, withRunners(0, 0)),
    ),
    "Spawn cache hits",
  );
  assert.equal(noop.right, "0 spawns (-10 spawns)");
  assert.equal(noop.status, "changed");

  const absent = row(
    comparisonRows(
      invocation({ id: "base" }, withRunners(9, 1)),
      invocation(
        { id: "cur" },
        { status: "ok", data: { targets: [], tests: [], truncated: false } },
      ),
    ),
    "Spawn cache hits",
  );
  assert.equal(absent.right, "Not recorded");
  assert.equal(absent.status, "unknown");
});

test("tests row summarizes pass/fail counts, lists new failures, and omits non-test builds", () => {
  const withTests = (tests: Array<{ label: string; status: string }>): Section<Bep> => ({
    status: "ok",
    data: { targets: [], tests, truncated: false },
  });
  const cmp = (
    a: Array<{ label: string; status: string }>,
    b: Array<{ label: string; status: string }>,
  ) =>
    comparisonRows(
      invocation({ id: "base" }, withTests(a)),
      invocation({ id: "cur" }, withTests(b)),
    );

  assert.equal(
    cmp([], []).find((r) => r.label === "Tests"),
    undefined,
  );

  const regressed = row(
    cmp(
      [
        { label: "//app:a_test", status: "PASSED" },
        { label: "//app:b_test", status: "PASSED" },
      ],
      [
        { label: "//app:a_test", status: "PASSED" },
        { label: "//app:b_test", status: "FAILED" },
      ],
    ),
    "Tests",
  );
  assert.equal(regressed.left, "2/2 passed");
  assert.equal(regressed.right, "1/2 passed (1 failed) [+1 failed]\nNew failures:\n//app:b_test");
  assert.equal(regressed.status, "changed");

  assert.equal(
    row(
      cmp(
        [{ label: "//app:b_test", status: "TIMEOUT" }],
        [{ label: "//app:b_test", status: "PASSED" }],
      ),
      "Tests",
    ).right,
    "1/1 passed [1 fixed]",
  );

  const flaky = row(
    cmp(
      [{ label: "//app:b_test", status: "FAILED" }],
      [{ label: "//app:b_test", status: "FLAKY" }],
    ),
    "Tests",
  );
  assert.equal(flaky.right, "0/1 passed (1 flaky)");

  const halted = row(
    cmp(
      [{ label: "//app:a_test", status: "PASSED" }],
      [{ label: "//app:a_test", status: "TOOL_HALTED_BEFORE_TESTING" }],
    ),
    "Tests",
  );
  assert.equal(halted.right, "0/1 passed (1 incomplete)");
});

test("inline diff applies to options/text but not IDs or dates", () => {
  const rows = comparisonRows(
    invocation(
      { id: "base-id-111", startedAt: 1000, pattern: "//😀:target" },
      options({ cmdLine: ["--jobs=8"] }),
    ),
    invocation(
      { id: "curr-id-222", startedAt: 9000, pattern: "//😃:target" },
      options({ cmdLine: ["--jobs=16"] }),
    ),
  );
  assert.equal(changedText(row(rows, "Full command line").leftParts), "8");
  assert.equal(changedText(row(rows, "Full command line").rightParts), "16");
  assert.ok(row(rows, "Invocation ID").leftParts.every((p) => !p.changed));
  assert.ok(row(rows, "Start date").rightParts.every((p) => !p.changed));

  const unpaired = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
  const pat = row(rows, "Pattern");
  assert.ok(pat.leftParts.every((p) => !unpaired.test(p.text)));
  assert.ok(pat.rightParts.every((p) => !unpaired.test(p.text)));
});

test("unloaded sides render placeholders without highlights", () => {
  const detail = invocation({ id: "r", cmd: "test" });
  assert.equal(row(comparisonRows(null, detail), "Command").left, "—");
  assert.equal(row(comparisonRows(detail, null), "Command").right, "—");
});

test("previousSuccessfulInvocation, recentComparableInvocations, and formatInvocationOption", () => {
  assert.equal(normalizePattern("//b  //a"), "//a //b");
  assert.equal(normalizePattern("//...  -//slow/..."), "//... -//slow/...");
  assert.notEqual(normalizePattern("//... -//slow/..."), normalizePattern("-//slow/... //..."));

  const rec = (id: string, extra: Partial<InvocationRecord>): InvocationRecord => ({
    id,
    workspace: "/repo/wt-a",
    cmd: "build",
    pattern: "//a //b",
    exitCode: 0,
    ...extra,
  });
  const current = rec("cur", { pattern: "//b //a", startedAt: 5000, exitCode: 1 });
  const history = [
    rec("newer", { startedAt: 6000 }),
    current,
    rec("other-wt", { workspace: "/repo/wt-b", startedAt: 4800 }),
    rec("failed", { startedAt: 4500, exitCode: 1 }),
    rec("running", { startedAt: 4200 }),
    rec("other-cmd", { cmd: "test", startedAt: 4100 }),
    rec("same-wt-ok", { startedAt: 3500 }),
  ];

  assert.equal(previousSuccessfulInvocation(current, history, ["running"])?.id, "same-wt-ok");
  assert.equal(
    previousSuccessfulInvocation({ ...current, workspace: "/repo/wt-c" }, history, ["running"])?.id,
    "other-wt",
  );
  assert.equal(
    previousSuccessfulInvocation({ id: "cur", cmd: "build", startedAt: 5000 }, history, []),
    null,
  );

  const grouped = recentComparableInvocations(current, history, ["running"]);
  assert.deepEqual(
    grouped.sameTarget.map((r) => r.id),
    ["same-wt-ok", "newer", "other-wt", "failed"],
  );
  assert.deepEqual(
    grouped.other.map((r) => r.id),
    ["other-cmd"],
  );

  const sample = rec("abc12345-6789", { pattern: "//app:all", startedAt: 1000, finishedAt: 2500 });
  assert.ok(formatInvocationOption(sample, true).startsWith("Previous successful (✓) · 1.500s · "));
  assert.ok(formatInvocationOption(sample, false).startsWith("✓ build //app:all · 1.500s · "));

  assert.deepEqual(
    filterBaselineOptions("", grouped, "same-wt-ok").map((o) => [o.group, o.id]),
    [
      ["Same target", "same-wt-ok"],
      ["Same target", "newer"],
      ["Same target", "other-wt"],
      ["Same target", "failed"],
      ["Other recent", "other-cmd"],
    ],
  );
  assert.deepEqual(
    filterBaselineOptions("other-cmd", grouped, "same-wt-ok").map((o) => [o.group, o.id]),
    [["Other recent", "other-cmd"]],
  );
  assert.deepEqual(
    filterBaselineOptions("00000000-0000-4000-8000-0000000000AB", grouped, "same-wt-ok").map(
      (o) => [o.group, o.id],
    ),
    [["Direct ID", "00000000-0000-4000-8000-0000000000ab"]],
  );
  assert.deepEqual(filterBaselineOptions("//no/such:target", grouped, "same-wt-ok"), []);
});
