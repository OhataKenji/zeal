import { test } from "node:test";
import assert from "node:assert/strict";
import {
  comparisonRows,
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
    data: { targets: [], tests: [], truncated: false, options: merged },
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
  assert.equal(cache.delta, undefined);
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

test("duration row reports current - baseline delta and keeps cells plain", () => {
  const slower = row(
    comparisonRows(
      invocation({ id: "base", startedAt: 1000, finishedAt: 3500 }),
      invocation({ id: "cur", startedAt: 1000, finishedAt: 6000 }),
    ),
    "Duration",
  );
  assert.equal(slower.left, "2.500s");
  assert.equal(slower.right, "5.000s");
  assert.deepEqual(slower.delta, { text: "+2.500s", tone: "worse" });
  assert.ok(slower.leftParts.every((p) => !p.changed));
  assert.ok(slower.rightParts.every((p) => !p.changed));

  const faster = row(
    comparisonRows(
      invocation({ id: "base", startedAt: 1000, finishedAt: 3500 }),
      invocation({ id: "cur", startedAt: 1000, finishedAt: 2000 }),
    ),
    "Duration",
  );
  assert.deepEqual(faster.delta, { text: "-1.500s", tone: "better" });

  const same = row(
    comparisonRows(
      invocation({ id: "base", startedAt: 5000, finishedAt: 6000 }),
      invocation({ id: "cur", startedAt: 1000, finishedAt: 2000 }),
    ),
    "Duration",
  );
  assert.deepEqual(same.delta, { text: "0.000s", tone: "same" });

  const missing = row(
    comparisonRows(
      invocation({ id: "base", startedAt: 1000 }),
      invocation({ id: "cur", startedAt: 2500, finishedAt: 1000 }),
    ),
    "Duration",
  );
  assert.equal(missing.status, "unknown");
  assert.equal(missing.delta, undefined);
});

test("spawn cache hits handles equal totals, changed totals, and 0-spawn no-op builds", () => {
  const sameTotalDrop = row(
    comparisonRows(
      invocation({ id: "base" }, withRunners(9, 1)),
      invocation({ id: "cur" }, withRunners(2, 8)),
    ),
    "Spawn cache hits",
  );
  assert.equal(sameTotalDrop.left, "90% (9/10)");
  assert.equal(sameTotalDrop.right, "20% (2/10)");
  assert.deepEqual(sameTotalDrop.delta, { text: "-70 pp", tone: "worse" });
  assert.ok(sameTotalDrop.leftParts.every((p) => !p.changed));
  assert.ok(sameTotalDrop.rightParts.every((p) => !p.changed));

  const sameTotalGain = row(
    comparisonRows(
      invocation({ id: "base" }, withRunners(5, 5)),
      invocation({ id: "cur" }, withRunners(10, 0)),
    ),
    "Spawn cache hits",
  );
  assert.deepEqual(sameTotalGain.delta, { text: "+50 pp", tone: "better" });

  // Incremental build (0/2) vs clean cached build (95/100) must stay neutral and show spawn diff.
  const incremental = row(
    comparisonRows(
      invocation({ id: "base" }, withRunners(95, 5)),
      invocation({ id: "cur" }, withRunners(0, 2)),
    ),
    "Spawn cache hits",
  );
  assert.deepEqual(incremental.delta, { text: "-95 pp (-98 spawns)", tone: "same" });

  // No-op build (0 spawns) is known, not "Not recorded".
  const noop = row(
    comparisonRows(
      invocation({ id: "base" }, withRunners(9, 1)),
      invocation({ id: "cur" }, withRunners(0, 0)),
    ),
    "Spawn cache hits",
  );
  assert.equal(noop.right, "0 spawns");
  assert.equal(noop.status, "changed");
  assert.deepEqual(noop.delta, { text: "-10 spawns", tone: "same" });
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
  const full = row(rows, "Full command line");
  assert.equal(changedText(full.leftParts), "8");
  assert.equal(changedText(full.rightParts), "16");

  const idRow = row(rows, "Invocation ID");
  assert.equal(idRow.status, "changed");
  assert.ok(idRow.leftParts.every((p) => !p.changed));
  assert.ok(idRow.rightParts.every((p) => !p.changed));

  const dateRow = row(rows, "Start date");
  assert.ok(dateRow.leftParts.every((p) => !p.changed));
  assert.ok(dateRow.rightParts.every((p) => !p.changed));

  // Surrogate pairs remain intact in pattern diff.
  const unpaired = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
  const pat = row(rows, "Pattern");
  assert.ok(pat.leftParts.every((part) => !unpaired.test(part.text)));
  assert.ok(pat.rightParts.every((part) => !unpaired.test(part.text)));
});

test("unloaded sides render placeholders without highlights", () => {
  const detail = invocation({ id: "r", cmd: "test" });
  const baseMissing = row(comparisonRows(null, detail), "Command");
  assert.equal(baseMissing.status, "unknown");
  assert.equal(baseMissing.left, "—");
  assert.equal(baseMissing.right, "test");

  const curMissing = row(comparisonRows(detail, null), "Command");
  assert.equal(curMissing.status, "unknown");
  assert.equal(curMissing.left, "test");
  assert.equal(curMissing.right, "—");
});

test("previousSuccessfulInvocation normalizes positive pattern order, preserves exclusions, and prefers same workspace", () => {
  assert.equal(normalizePattern("//b  //a"), "//a //b");
  assert.equal(normalizePattern("//...  -//slow/..."), "//... -//slow/...");
  assert.notEqual(normalizePattern("//... -//slow/..."), normalizePattern("-//slow/... //..."));

  const current: InvocationRecord = {
    id: "cur",
    workspace: "/repo/wt-a",
    cmd: "build",
    pattern: "//b //a",
    startedAt: 5000,
    exitCode: 1,
  };
  const history: InvocationRecord[] = [
    {
      id: "newer",
      workspace: "/repo/wt-a",
      cmd: "build",
      pattern: "//a //b",
      startedAt: 6000,
      exitCode: 0,
    },
    current,
    {
      id: "other-wt-newer",
      workspace: "/repo/wt-b",
      cmd: "build",
      pattern: "//a //b",
      startedAt: 4800,
      exitCode: 0,
    },
    {
      id: "failed",
      workspace: "/repo/wt-a",
      cmd: "build",
      pattern: "//a //b",
      startedAt: 4500,
      exitCode: 1,
    },
    {
      id: "running-prev",
      workspace: "/repo/wt-a",
      cmd: "build",
      pattern: "//a //b",
      startedAt: 4200,
      exitCode: 0,
    },
    {
      id: "other-cmd",
      workspace: "/repo/wt-a",
      cmd: "test",
      pattern: "//a //b",
      startedAt: 4100,
      exitCode: 0,
    },
    {
      id: "same-wt-ok",
      workspace: "/repo/wt-a",
      cmd: "build",
      pattern: "//a //b",
      startedAt: 3500,
      exitCode: 0,
    },
  ];

  // Prefers same workspace even when another worktree has a newer match.
  assert.equal(previousSuccessfulInvocation(current, history, ["running-prev"])?.id, "same-wt-ok");

  // Falls back to another worktree when same workspace has no match.
  assert.equal(
    previousSuccessfulInvocation({ ...current, workspace: "/repo/wt-c" }, history, ["running-prev"])
      ?.id,
    "other-wt-newer",
  );

  assert.equal(
    previousSuccessfulInvocation({ id: "cur", cmd: "build", startedAt: 5000 }, history, []),
    null,
  );
});

test("recentComparableInvocations partitions same-target and other settled records", () => {
  const current: InvocationRecord = { id: "cur", cmd: "build", pattern: "//b //a" };
  const history: InvocationRecord[] = [
    { id: "run-1", cmd: "build", pattern: "//a //b" },
    current,
    { id: "same-1", cmd: "build", pattern: "//a //b" },
    { id: "other-1", cmd: "test", pattern: "//a //b" },
  ];
  const grouped = recentComparableInvocations(current, history, ["run-1"]);
  assert.deepEqual(
    grouped.sameTarget.map((r) => r.id),
    ["same-1"],
  );
  assert.deepEqual(
    grouped.other.map((r) => r.id),
    ["other-1"],
  );
});

test("formatInvocationOption labels previous successful and ordinary builds", () => {
  const rec: InvocationRecord = {
    id: "abc12345-6789",
    cmd: "build",
    pattern: "//app:all",
    startedAt: 1000,
    finishedAt: 2500,
    exitCode: 0,
  };
  assert.ok(formatInvocationOption(rec, true).startsWith("Previous successful (✓) · 1.500s · "));
  assert.ok(formatInvocationOption(rec, false).startsWith("✓ build //app:all · 1.500s · "));
});
