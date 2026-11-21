import { test } from "node:test";
import assert from "node:assert/strict";
import { pages } from "../src/lib/pages.ts";
import { createIndex } from "../src/lib/search.ts";
import { paletteItems, type PaletteInput } from "../src/lib/palette.ts";
import type { InvocationRecord, OwnedInvocation, Target } from "../src/lib/types.ts";

function target(label: string, ruleClass = "cc_library"): Target {
  const [pkg, name] = label.slice(2).split(":");
  return { label, pkg, name, ruleClass, location: null };
}

// Ten labels share the literal "match" substring so a query caps at 8 rows.
const TARGETS: Target[] = [
  target("//app:server", "java_binary"),
  target("//app/lib:parser", "java_library"),
  target("//app/lib:parser_test", "java_test"),
  ...Array.from({ length: 10 }, (_, index) =>
    target(`//bulk/pkg${index}:match${index}`, "genrule"),
  ),
];

const INDEX = createIndex(TARGETS);

function record(id: string, over: Partial<InvocationRecord> = {}): InvocationRecord {
  return {
    id,
    workspace: "/workspace",
    cmd: "build",
    pattern: "//app:server",
    startedAt: 1,
    finishedAt: 2,
    exitCode: 0,
    ...over,
  };
}

function row(id: string, over: Partial<InvocationRecord> = {}, execution?: OwnedInvocation) {
  return execution === undefined
    ? { record: record(id, over) }
    : { record: record(id, over), execution };
}

const KNOWN_UUID = "123e4567-e89b-42d3-a456-426614174000";
const ROWS = [
  row(KNOWN_UUID),
  row("bbbbbbbb-2222-4222-8222-222222222222", {
    cmd: "test",
    pattern: "//app/lib:parser_test",
    exitCode: 1,
  }),
  row("run-c"),
  row("run-d"),
  row("run-e"),
  row("run-f"),
  row("run-g"),
];

function input(over: Partial<PaletteInput> = {}): PaletteInput {
  return {
    query: "",
    targetIndex: INDEX,
    invocations: ROWS,
    canRun: true,
    canRefresh: true,
    isMac: true,
    ...over,
  };
}

function labels(items: ReturnType<typeof paletteItems>, group?: string): string[] {
  return items.filter((item) => group === undefined || item.group === group).map((i) => i.label);
}

test("empty and whitespace queries return commands plus the five most recent invocations", () => {
  for (const query of ["", "   "]) {
    const items = paletteItems(input({ query }));
    assert.deepEqual(items.find((item) => item.label === "Go to Modules")?.action, {
      kind: "page",
      page: "modules",
    });
    // Every command shows for an empty query — pages plus the specials —
    // so adding a rail page must not be silently capped at eight.
    assert.deepEqual(labels(items, "commands"), [
      ...pages.map((page) => `Go to ${page.label}`),
      "Run Bazel…",
      "Reload targets",
      "Open workspace…",
    ]);
    const invocations = items.filter((item) => item.group === "invocations");
    assert.deepEqual(
      invocations.map((item) => item.action),
      ROWS.slice(0, 5).map((entry) => ({ kind: "invocation", id: entry.record.id })),
    );
    assert.equal(
      items.some((item) => item.group === "targets"),
      false,
    );
  }
});

test("canRun and canRefresh gate their commands", () => {
  const items = paletteItems(input({ canRun: false, canRefresh: false }));
  const commands = labels(items, "commands");
  assert.equal(commands.includes("Run Bazel…"), false);
  assert.equal(commands.includes("Reload targets"), false);
  assert.equal(commands.includes("Open workspace…"), true);
});

test("shortcut labels follow the platform", () => {
  const mac = paletteItems(input({ isMac: true }));
  assert.equal(mac.find((item) => item.label === "Run Bazel…")?.shortcut, "⌘⇧R");
  assert.equal(mac.find((item) => item.label === "Open workspace…")?.shortcut, "⌘O");
  const other = paletteItems(input({ isMac: false }));
  assert.equal(other.find((item) => item.label === "Run Bazel…")?.shortcut, "Ctrl+Shift+R");
  assert.equal(other.find((item) => item.label === "Open workspace…")?.shortcut, "Ctrl+O");
});

test("a text query filters each group and caps targets and invocations at eight", () => {
  const items = paletteItems(input({ query: "match" }));
  assert.deepEqual(labels(items, "commands"), []);
  assert.equal(items.filter((item) => item.group === "targets").length, 8);
  const invocationItems = items.filter((item) => item.group === "invocations");
  assert.equal(invocationItems.length, 0);

  const manyRows = Array.from({ length: 10 }, (_, index) =>
    row(`match-run-${index}`, { pattern: `//x:match${index}` }),
  );
  const capped = paletteItems(input({ query: "match", invocations: manyRows }));
  assert.equal(capped.filter((item) => item.group === "invocations").length, 8);
});

test("commands match on label and keywords case-insensitively", () => {
  assert.deepEqual(labels(paletteItems(input({ query: "BUILD" })), "commands"), ["Run Bazel…"]);
  const workspace = labels(paletteItems(input({ query: "workspace" })), "commands");
  assert.deepEqual(workspace.sort(), ["Go to Workspace", "Open workspace…"].sort());
});

test("a null target index still returns commands and invocations", () => {
  const items = paletteItems(input({ query: "parser", targetIndex: null }));
  assert.equal(
    items.some((item) => item.group === "targets"),
    false,
  );
  assert.ok(items.some((item) => item.group === "invocations"));

  const commands = paletteItems(input({ query: "build", targetIndex: null }));
  assert.ok(commands.some((item) => item.group === "commands"));
  assert.ok(commands.some((item) => item.group === "invocations"));
  assert.equal(
    commands.some((item) => item.group === "targets"),
    false,
  );
});

test("invocation ids, patterns and commands match as substrings", () => {
  const items = paletteItems(input({ query: "parser_test" }));
  const found = items.filter((item) => item.group === "invocations");
  assert.equal(found.length, 1);
  assert.equal(found[0]?.action.kind === "invocation" && found[0].action.id, ROWS[1].record.id);
  assert.match(found[0]?.detail ?? "", new RegExp(ROWS[1].record.id.slice(0, 8)));

  const byId = paletteItems(input({ query: "run-c" }));
  assert.ok(byId.some((item) => item.id === "invocation:run-c"));
});

test("an unknown UUID appends one lowercase-normalized lookup item", () => {
  const unknown = "A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11";
  const items = paletteItems(input({ query: unknown }));
  const lookup = items.filter((item) => item.detail === "Look up invocation ID");
  assert.equal(lookup.length, 1);
  assert.equal(lookup[0]?.label, unknown.toLowerCase());
  assert.deepEqual(lookup[0]?.action, {
    kind: "invocation",
    id: unknown.toLowerCase(),
  });
  assert.equal(items.at(-1), lookup[0]);
});

test("a UUID already in history does not duplicate the lookup item", () => {
  const items = paletteItems(input({ query: KNOWN_UUID.toUpperCase() }));
  assert.equal(items.filter((item) => item.detail === "Look up invocation ID").length, 0);
  const found = items.filter((item) => item.id === `invocation:${KNOWN_UUID}`);
  assert.equal(found.length, 1);
});

test("an exact UUID id is promoted ahead of rows that only contain it", () => {
  const fillers = Array.from({ length: 8 }, (_, index) =>
    row(`fill-${index}`, { pattern: `//pkg:file-${KNOWN_UUID}` }),
  );
  const items = paletteItems(
    input({ query: KNOWN_UUID, invocations: [...fillers, row(KNOWN_UUID)] }),
  );
  const invocations = items.filter((item) => item.group === "invocations");
  assert.equal(invocations.length, 8);
  assert.deepEqual(invocations[0]?.action, { kind: "invocation", id: KNOWN_UUID });
  assert.equal(invocations[1]?.action.kind === "invocation" && invocations[1].action.id, "fill-0");
  assert.equal(
    items.some((item) => item.detail === "Look up invocation ID"),
    false,
  );
});

test("ordinary words never produce a lookup item", () => {
  for (const query of ["build", "parser", "//app:server", "123e4567"]) {
    const items = paletteItems(input({ query }));
    assert.equal(items.filter((item) => item.detail === "Look up invocation ID").length, 0, query);
  }
});

test("queries are truncated to 512 characters before matching", () => {
  // The first 512 chars are all spaces, so "match" falls outside the window
  // and the trimmed query is empty: only the empty-query groups return.
  const items = paletteItems(input({ query: `${" ".repeat(600)}match` }));
  assert.equal(
    items.some((item) => item.group === "targets"),
    false,
  );
  assert.ok(items.some((item) => item.group === "commands"));
  assert.ok(items.some((item) => item.group === "invocations"));
});
