import { test } from "node:test";
import assert from "node:assert/strict";
import { comparisonRows } from "../src/lib/invocation-diff.ts";
import type { ComparisonInvocation } from "../src/lib/invocation-diff.ts";
import type { Bep, InvocationRecord, Section } from "../src/lib/types.ts";

const OPTS: NonNullable<Bep["options"]> = {
  startupOptions: ["--startup-full"],
  explicitStartupOptions: ["--startup-explicit"],
  cmdLine: ["--cmd-full", "value with space"],
  explicitCmdLine: ["--cmd-explicit"],
};

function bepSection(status: Section<Bep>["status"] = "ok", options?: Bep["options"]): Section<Bep> {
  const data: Bep = { targets: [], tests: [], truncated: false, options };
  switch (status) {
    case "ok":
      return { status, data };
    case "incomplete":
      return { status, data, note: "truncated" };
    case "missing":
      return { status };
    default:
      return { status, note: "unavailable" };
  }
}

function invocation(
  record: Partial<InvocationRecord> & Pick<InvocationRecord, "id">,
  bep: Section<Bep> = bepSection("ok", OPTS),
): ComparisonInvocation {
  return { record: { ...record }, bep };
}

function row(rows: ReturnType<typeof comparisonRows>, label: string) {
  const found = rows.find((candidate) => candidate.label === label);
  assert.ok(found, `missing row ${label}`);
  return found;
}

test("comparisonRows returns the ten facet rows in order", () => {
  const rows = comparisonRows(invocation({ id: "a" }), invocation({ id: "b" }));
  assert.deepEqual(
    rows.map((candidate) => candidate.label),
    [
      "Invocation ID",
      "Command",
      "Pattern",
      "Exit code",
      "Start date",
      "Duration",
      "Explicit command line",
      "Full command line",
      "Explicit startup options",
      "Full startup options",
    ],
  );
});

test("compares record fields and options independently", () => {
  const left = invocation({
    id: "left-id",
    cmd: "build",
    pattern: "//app:all",
    exitCode: 0,
    startedAt: 1700000000000,
    finishedAt: 1700000001000,
  });
  const right = invocation(
    {
      id: "right-id",
      cmd: "build",
      pattern: "//app:all",
      exitCode: 1,
      startedAt: 1700000000000,
      finishedAt: 1700000002000,
    },
    bepSection("ok", { ...OPTS, cmdLine: ["--cmd-full", "other"] }),
  );
  const rows = comparisonRows(left, right);
  assert.equal(row(rows, "Invocation ID").status, "changed");
  assert.equal(row(rows, "Command").status, "same");
  assert.equal(row(rows, "Exit code").status, "changed");
  assert.equal(row(rows, "Duration").left, "1.000s");
  assert.equal(row(rows, "Duration").right, "2.000s");
  assert.equal(row(rows, "Duration").status, "changed");
  assert.equal(row(rows, "Explicit command line").status, "same");
  assert.equal(row(rows, "Full command line").status, "changed");
});

test("compares option arrays by exact order and duplicates", () => {
  const withCmdLine = (cmdLine: string[]) => ({ ...OPTS, cmdLine });
  const reordered = comparisonRows(
    invocation({ id: "x" }, bepSection("ok", withCmdLine(["a", "b", "a"]))),
    invocation({ id: "x" }, bepSection("ok", withCmdLine(["b", "a", "a"]))),
  );
  assert.equal(row(reordered, "Full command line").status, "changed");
  const identical = comparisonRows(
    invocation({ id: "x" }, bepSection("ok", withCmdLine(["a", "b", "a"]))),
    invocation({ id: "x" }, bepSection("ok", withCmdLine(["a", "b", "a"]))),
  );
  const full = row(identical, "Full command line");
  assert.equal(full.status, "same");
  assert.equal(full.left, "a\nb\na");
});

test("distinguishes empty option arrays from absent options", () => {
  const rows = comparisonRows(
    invocation({ id: "x" }, bepSection("ok", { ...OPTS, cmdLine: [] })),
    invocation({ id: "x" }, bepSection("ok")),
  );
  const full = row(rows, "Full command line");
  assert.equal(full.left, "No options");
  assert.equal(full.right, "Not recorded");
  assert.equal(full.status, "unknown");

  const nonOk = comparisonRows(
    invocation({ id: "x", cmd: "build" }),
    invocation({ id: "x", cmd: "build" }, bepSection("missing")),
  );
  assert.equal(row(nonOk, "Full command line").status, "unknown");
  assert.equal(row(nonOk, "Command").status, "same");
});

test("identical invocations report every row as same", () => {
  const fixture = () =>
    invocation({
      id: "same",
      cmd: "build",
      pattern: "//app:all",
      exitCode: 0,
      startedAt: 1700000000000,
      finishedAt: 1700000001000,
    });
  const rows = comparisonRows(fixture(), fixture());
  assert.ok(rows.every((candidate) => candidate.status === "same"));
});

test("duration is known only with both endpoints and a nonnegative result", () => {
  const withTimes = (startedAt?: number, finishedAt?: number) =>
    invocation({ id: "x", startedAt, finishedAt });

  let rows = comparisonRows(withTimes(1000, 2500), withTimes(1000, 2500));
  assert.equal(row(rows, "Duration").left, "1.500s");
  assert.equal(row(rows, "Duration").status, "same");

  rows = comparisonRows(withTimes(1000, undefined), withTimes(1000, 2500));
  assert.equal(row(rows, "Duration").left, "Not recorded");
  assert.equal(row(rows, "Duration").status, "unknown");

  rows = comparisonRows(withTimes(2500, 1000), withTimes(2500, 1000));
  assert.equal(row(rows, "Duration").left, "Not recorded");
  assert.equal(row(rows, "Duration").status, "unknown");

  rows = comparisonRows(withTimes(1000, 1000), withTimes(1000, 1000));
  assert.equal(row(rows, "Duration").left, "0.000s");
  assert.equal(row(rows, "Duration").status, "same");
});

test("status uses raw values even when formatted display collides", () => {
  const rows = comparisonRows(
    invocation({ id: "x", startedAt: 1700000000100, finishedAt: 1700000001100 }),
    invocation({ id: "x", startedAt: 1700000000200, finishedAt: 1700000001200 }),
  );
  const start = row(rows, "Start date");
  assert.equal(start.status, "changed");
  assert.equal(start.left, start.right);
});

test("option display escapes arguments while equality stays raw", () => {
  const rows = comparisonRows(
    invocation({ id: "x" }, bepSection("ok", { ...OPTS, cmdLine: ["it's", "--flag=a b", ""] })),
    invocation({ id: "x" }, bepSection("ok", { ...OPTS, cmdLine: ["it's", "--flag=a b", "''"] })),
  );
  const full = row(rows, "Full command line");
  assert.equal(full.status, "changed");
  assert.ok(full.left.split("\n").includes("''"));
  assert.ok(full.left.includes("'it'\\''s'"));
});

test("unloaded sides render placeholders and unknown status", () => {
  const right = invocation({ id: "r", cmd: "test" });
  const rows = comparisonRows(null, right);
  assert.ok(rows.every((candidate) => candidate.status === "unknown"));
  assert.ok(rows.every((candidate) => candidate.left === "—"));
  assert.equal(row(rows, "Command").right, "test");

  const mirrored = comparisonRows(right, null);
  assert.ok(mirrored.every((candidate) => candidate.right === "—"));
  assert.ok(mirrored.every((candidate) => candidate.status === "unknown"));
  assert.equal(row(mirrored, "Command").left, "test");
});
