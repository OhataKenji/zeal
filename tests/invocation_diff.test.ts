import { test } from "node:test";
import assert from "node:assert/strict";
import { comparisonRows } from "../src/lib/invocation-diff.ts";
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

test("maps each facet to its record or BEP source", () => {
  const detail = invocation(
    {
      id: "run",
      cmd: "build",
      pattern: "//app:all",
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
      ["Invocation ID", "run", "run"],
      ["Command", "build", "build"],
      ["Pattern", "//app:all", "//app:all"],
      ["Exit code", "0", "0"],
      ["Start date", new Date(1000).toLocaleString(), new Date(1000).toLocaleString()],
      ["Duration", "1.500s", "1.500s"],
      ["Explicit command line", "--cmd-explicit", "--cmd-explicit"],
      ["Full command line", "--cmd-full", "--cmd-full"],
      ["Explicit startup options", "--startup-explicit", "--startup-explicit"],
      ["Full startup options", "--startup-full", "--startup-full"],
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

test("incomplete BEP data leaves option rows unknown and plain", () => {
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
  assert.ok(full.leftParts.every((part) => !part.changed));
  assert.ok(full.rightParts.every((part) => !part.changed));
});

test("option arrays preserve order and duplicates", () => {
  const rows = comparisonRows(
    invocation({ id: "x" }, options({ cmdLine: ["a", "b", "a"] })),
    invocation({ id: "x" }, options({ cmdLine: ["b", "a", "a"] })),
  );
  const full = row(rows, "Full command line");
  assert.equal(full.status, "changed");
  assert.equal(full.left, "a\nb\na");
  assert.equal(full.right, "b\na\na");
});

test("option arrays compare element counts", () => {
  const rows = comparisonRows(
    invocation({ id: "x" }, options({ cmdLine: ["a", "a"] })),
    invocation({ id: "x" }, options({ cmdLine: ["a"] })),
  );
  assert.equal(row(rows, "Full command line").status, "changed");
});

test("empty option arrays differ from absent options", () => {
  const rows = comparisonRows(
    invocation({ id: "x" }, options({ cmdLine: [] })),
    invocation({ id: "x" }, { status: "ok", data: { targets: [], tests: [], truncated: false } }),
  );
  const full = row(rows, "Full command line");
  assert.equal(full.left, "No options");
  assert.equal(full.right, "Not recorded");
  assert.equal(full.status, "unknown");
  assert.ok(full.leftParts.every((part) => !part.changed));
  assert.ok(full.rightParts.every((part) => !part.changed));
});

test("empty options highlight only the recorded side", () => {
  const rows = comparisonRows(
    invocation({ id: "x" }, options({ cmdLine: [] })),
    invocation({ id: "x" }, options({ cmdLine: ["--only"] })),
  );
  const full = row(rows, "Full command line");
  assert.equal(full.status, "changed");
  assert.deepEqual(full.leftParts, [{ text: "No options", changed: false }]);
  assert.deepEqual(full.rightParts, [{ text: "--only", changed: true }]);
});

test("duration is unknown when an endpoint is missing", () => {
  const rows = comparisonRows(
    invocation({ id: "x", startedAt: 1000 }),
    invocation({ id: "x", finishedAt: 2500 }),
  );
  const duration = row(rows, "Duration");
  assert.equal(duration.left, "Not recorded");
  assert.equal(duration.right, "Not recorded");
  assert.equal(duration.status, "unknown");
});

test("duration is unknown for a negative span", () => {
  const rows = comparisonRows(
    invocation({ id: "x", startedAt: 2500, finishedAt: 1000 }),
    invocation({ id: "x", startedAt: 1000, finishedAt: 2500 }),
  );
  const duration = row(rows, "Duration");
  assert.equal(duration.left, "Not recorded");
  assert.equal(duration.status, "unknown");
});

test("zero duration is a known value", () => {
  const rows = comparisonRows(
    invocation({ id: "x", startedAt: 1000, finishedAt: 1000 }),
    invocation({ id: "x", startedAt: 1000, finishedAt: 1001 }),
  );
  const duration = row(rows, "Duration");
  assert.equal(duration.left, "0.000s");
  assert.equal(duration.right, "0.001s");
  assert.equal(duration.status, "changed");
});

test("raw differences with colliding display stay unhighlighted", () => {
  const rows = comparisonRows(
    invocation({ id: "x", startedAt: 1700000000100, finishedAt: 1700000001100 }),
    invocation({ id: "x", startedAt: 1700000000200, finishedAt: 1700000001200 }),
  );
  const start = row(rows, "Start date");
  assert.equal(start.status, "changed");
  assert.equal(start.left, start.right);
  assert.ok(start.leftParts.every((part) => !part.changed));
  assert.ok(start.rightParts.every((part) => !part.changed));
});

test("changed rows mark only the differing characters", () => {
  const rows = comparisonRows(
    invocation({ id: "x" }, options({ cmdLine: ["--jobs=8"] })),
    invocation({ id: "x" }, options({ cmdLine: ["--jobs=16"] })),
  );
  const full = row(rows, "Full command line");
  assert.equal(full.status, "changed");
  assert.equal(changedText(full.leftParts), "8");
  assert.equal(changedText(full.rightParts), "16");
  assert.equal(text(full.leftParts), full.left);
  assert.equal(text(full.rightParts), full.right);
});

test("inline diff parts reconstruct multiline displays", () => {
  const rows = comparisonRows(
    invocation({ id: "x" }, options({ cmdLine: ["--a=1", "--b=2"] })),
    invocation({ id: "x" }, options({ cmdLine: ["--a=1", "--b=3", "--c=4"] })),
  );
  const full = row(rows, "Full command line");
  assert.equal(full.status, "changed");
  assert.equal(text(full.leftParts), full.left);
  assert.equal(text(full.rightParts), full.right);
  assert.ok(full.leftParts.some((part) => part.changed));
  assert.ok(full.rightParts.some((part) => part.changed));
});

test("raw values matching sentinel text still diff inline", () => {
  const rows = comparisonRows(
    invocation({ id: "x", pattern: "No options" }),
    invocation({ id: "x", pattern: "More options" }),
  );
  const pattern = row(rows, "Pattern");
  assert.equal(pattern.status, "changed");
  assert.ok(pattern.leftParts.some((part) => part.changed));
  assert.ok(pattern.rightParts.some((part) => part.changed));
  assert.equal(text(pattern.leftParts), "No options");
  assert.equal(text(pattern.rightParts), "More options");
});

test("diff parts keep surrogate pairs intact", () => {
  const unpaired = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
  const rows = comparisonRows(
    invocation({ id: "x", pattern: "//😀:target" }),
    invocation({ id: "x", pattern: "//😃:target" }),
  );
  const pattern = row(rows, "Pattern");
  assert.equal(pattern.status, "changed");
  assert.ok(pattern.leftParts.every((part) => !unpaired.test(part.text)));
  assert.ok(pattern.rightParts.every((part) => !unpaired.test(part.text)));
  assert.equal(text(pattern.leftParts), pattern.left);
  assert.equal(text(pattern.rightParts), pattern.right);
});

test("unloaded sides render placeholders without highlights", () => {
  const detail = invocation({ id: "r", cmd: "test" });

  const leftMissing = row(comparisonRows(null, detail), "Command");
  assert.equal(leftMissing.status, "unknown");
  assert.equal(leftMissing.left, "—");
  assert.equal(leftMissing.right, "test");
  assert.ok(leftMissing.leftParts.every((part) => !part.changed));
  assert.ok(leftMissing.rightParts.every((part) => !part.changed));

  const rightMissing = row(comparisonRows(detail, null), "Command");
  assert.equal(rightMissing.status, "unknown");
  assert.equal(rightMissing.right, "—");
  assert.equal(rightMissing.left, "test");
  assert.ok(rightMissing.leftParts.every((part) => !part.changed));
  assert.ok(rightMissing.rightParts.every((part) => !part.changed));
});
