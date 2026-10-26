import { test } from "node:test";
import assert from "node:assert/strict";
import {
  LIST_ID,
  PATTERN_HINTS,
  createIndex,
  keyAction,
  moveIndex,
  optionId,
  search,
  segments,
} from "../src/lib/search.ts";
import type { Snapshot } from "../src/lib/types.ts";

const snap: Snapshot = {
  meta: { fetchedAt: 1, status: "ok" },
  targets: [
    {
      label: "//app/lib:parser",
      pkg: "app/lib",
      name: "parser",
      ruleClass: "java_library",
      location: null,
    },
    {
      label: "//app/lib:parser_test",
      pkg: "app/lib",
      name: "parser_test",
      ruleClass: "java_test",
      location: null,
    },
    {
      label: "//very/deep/package:some_long_target_name",
      pkg: "very/deep/package",
      name: "some_long_target_name",
      ruleClass: "genrule",
      location: null,
    },
  ],
  edges: [],
  files: [
    // Several owners: the ascending-first label is the second one seen.
    { path: "app/lib/Parser.java", owner: "//app/lib:zparser", role: "src" },
    { path: "app/lib/Parser.java", owner: "//app/lib:aparser", role: "src" },
    { path: "app/lib/parser.jar", owner: "//app/lib:parser", role: "out" },
    { path: "app/lib/Lexer.java", owner: "//app/lib:lexer", role: "src" },
  ],
};

// The index belongs to whoever holds the candidates: built once here, searched
// by every case below.
const index = createIndex(snap.targets);

const greeter = "//examples/cpp/helloworld:greeter_client";
const greeterIndex = createIndex(
  [greeter, "//other/cpp/helloworld:greeter_client"].map((label) => ({
    label,
    pkg: label.slice(2).split(":")[0],
    name: "greeter_client",
    ruleClass: "cc_binary",
    location: null,
  })),
);

for (const query of [
  "examplehellow",
  "example hellow",
  "example helloworld",
  "helloworld example",
  "exhw",
  "EXAMPLE HELLOWORLD",
]) {
  test(`target search matches all fragments in ${JSON.stringify(query)}`, () => {
    assert.deepEqual(
      search(greeterIndex, query).map((match) => match.target.label),
      [greeter],
    );
  });
}

test("subsequence matches retain the matching characters", () => {
  const match = search(greeterIndex, "examplehellow")[0];
  assert.ok(match);
  assert.equal(
    segments(match.target.label, match.indices)
      .filter((run) => run.hit)
      .map((run) => run.text)
      .join(""),
    "examplehellow",
  );
});

test("search results retain the target and its rule class", () => {
  const match = search(index, "app/lib:parser").find(
    (match) => match.target.label === "//app/lib:parser",
  );
  assert.ok(match);
  assert.deepEqual(match.target, snap.targets[0]);
});

test("file paths are not search candidates", () => {
  assert.deepEqual(search(index, "Lexer.java"), []);
  assert.deepEqual(search(index, "Parser.java"), []);
  assert.deepEqual(search(index, "parser.jar"), []);
});

test("later searches do not change earlier results", () => {
  const first = search(greeterIndex, "example");
  const saved = structuredClone(first);
  search(greeterIndex, "greeter");
  assert.deepEqual(first, saved);
  assert.deepEqual(search(greeterIndex, "example"), saved);
});

test("a query at the tail of a long label matches and highlights the tail", () => {
  const found = search(index, "target_name");
  assert.equal(found.length > 0, true);
  const top = found[0];
  assert.equal(top.target.label, "//very/deep/package:some_long_target_name");
  const hit = segments(top.target.label, top.indices)
    .filter((run) => run.hit)
    .map((run) => run.text)
    .join("");
  assert.equal(hit, "target_name");
  assert.equal(top.target.label.endsWith(hit), true);
});

test("an empty query searches nothing and the limit caps the rows", () => {
  assert.deepEqual(search(index, ""), []);
  assert.deepEqual(search(index, "   "), []);
  assert.equal(search(index, "parser", 1).length, 1);
  assert.equal(search(index, "parser", 2).length, 2);
  assert.equal(search(index, "parser", 0).length, 0);
  assert.deepEqual(search(index, "parser", -1), []);
  assert.equal(search(index, "parser").length, 2);
});

test("an index keeps answering its own queries, and another array makes another index", () => {
  const first = search(index, "parser");
  assert.equal(first.length > 0, true);
  assert.deepEqual(search(index, "parser"), first);

  // Index creation reads the target array, so a different snapshot needs a
  // different index rather than a search of the one above.
  const other = createIndex([
    {
      label: "//other:only_here",
      pkg: "other",
      name: "only_here",
      ruleClass: "genrule",
      location: null,
    },
  ]);
  assert.notEqual(other, index);
  assert.deepEqual(search(other, "parser"), []);
  assert.deepEqual(
    search(other, "only_here").map((match) => match.target.label),
    ["//other:only_here"],
  );
});

test("moveIndex moves and clamps inside the list", () => {
  assert.equal(moveIndex(0, 1, 3), 1);
  assert.equal(moveIndex(2, 1, 3), 2);
  assert.equal(moveIndex(0, -1, 3), 0);
  assert.equal(moveIndex(-4, 0, 3), 0);
  assert.equal(moveIndex(9, 0, 3), 2);
  assert.equal(moveIndex(0, 1, 0), 0);
  assert.equal(moveIndex(7, -5, 0), 0);
});

test("arrow keys open, move and clamp the candidate selection", () => {
  const matches = search(index, "parser");
  assert.equal(matches.length >= 2, true);
  const last = matches.length - 1;

  assert.deepEqual(keyAction({ open: false, index: 0 }, "ArrowDown", matches), {
    kind: "move",
    index: 0,
  });
  // A closed list always reopens at its first row, whatever was selected before.
  assert.deepEqual(keyAction({ open: false, index: last }, "ArrowDown", matches), {
    kind: "move",
    index: 0,
  });
  assert.deepEqual(keyAction({ open: true, index: 0 }, "ArrowDown", matches), {
    kind: "move",
    index: 1,
  });
  assert.deepEqual(keyAction({ open: true, index: last }, "ArrowDown", matches), {
    kind: "move",
    index: last,
  });
  assert.deepEqual(keyAction({ open: true, index: 1 }, "ArrowUp", matches), {
    kind: "move",
    index: 0,
  });
  assert.deepEqual(keyAction({ open: true, index: 0 }, "ArrowUp", matches), {
    kind: "move",
    index: 0,
  });
  assert.deepEqual(keyAction({ open: false, index: 3 }, "ArrowUp", matches), {
    kind: "move",
    index: 0,
  });
});

test("keyAction does nothing for arrows without matches", () => {
  assert.deepEqual(keyAction({ open: true, index: 0 }, "ArrowDown", []), {
    kind: "none",
  });
  assert.deepEqual(keyAction({ open: false, index: 0 }, "ArrowDown", []), {
    kind: "none",
  });
  assert.deepEqual(keyAction({ open: true, index: 0 }, "ArrowUp", []), {
    kind: "none",
  });
  assert.deepEqual(keyAction({ open: false, index: 0 }, "ArrowUp", []), {
    kind: "none",
  });
});

test("Enter confirms the highlighted candidate without requiring an arrow key", () => {
  const matches = search(index, "parser");
  assert.ok(matches.length >= 2);
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Enter", matches), {
    kind: "pick",
    value: matches[0].target.label,
  });
  assert.deepEqual(keyAction({ open: true, index: 1 }, "Enter", matches), {
    kind: "pick",
    value: matches[1].target.label,
  });
  assert.deepEqual(keyAction({ open: true, index: 99 }, "Enter", matches), {
    kind: "pick",
    value: matches.at(-1)!.target.label,
  });
});

test("Enter cannot execute after confirmation, after Escape or without candidates", () => {
  const matches = search(index, "parser");
  assert.deepEqual(keyAction({ open: false, index: 0 }, "Enter", matches), { kind: "none" });
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Enter", []), { kind: "none" });
  assert.deepEqual(keyAction({ open: false, index: 0 }, "Enter", []), { kind: "none" });
});

test("patterns without matching candidates remain available for Send", () => {
  const matches = search(index, "//app/...");
  assert.deepEqual(matches, []);
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Enter", matches), { kind: "none" });
  // Closing suggestions leaves the typed pattern for the separate Send action.
  assert.deepEqual(keyAction({ open: false, index: 0 }, "Enter", matches), { kind: "none" });
});

test("Escape closes only an open dropdown and other keys do nothing", () => {
  const matches = search(index, "parser");
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Escape", matches), {
    kind: "close",
  });
  assert.deepEqual(keyAction({ open: false, index: 0 }, "Escape", matches), {
    kind: "none",
  });
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Tab", matches), {
    kind: "none",
  });
  assert.deepEqual(keyAction({ open: true, index: 0 }, "a", matches), {
    kind: "none",
  });
});

test("the dropdown list and option ids come from one place", () => {
  assert.equal(LIST_ID, "qo-list");
  assert.equal(optionId(0), "qo-opt-0");
  assert.equal(optionId(12), "qo-opt-12");
});

test("segments merge ranges and cover the whole string", () => {
  assert.deepEqual(
    segments("abcdef", [
      [4, 5],
      [0, 1],
    ]),
    [
      { text: "ab", hit: true },
      { text: "cd", hit: false },
      { text: "ef", hit: true },
    ],
  );
  assert.deepEqual(
    segments("abcdef", [
      [0, 2],
      [1, 3],
    ]),
    [
      { text: "abcd", hit: true },
      { text: "ef", hit: false },
    ],
  );
  assert.deepEqual(
    segments("abcdef", [
      [0, 0],
      [1, 1],
    ]),
    [
      { text: "ab", hit: true },
      { text: "cdef", hit: false },
    ],
  );
  assert.deepEqual(segments("abc", [[0, 99]]), [{ text: "abc", hit: true }]);
  assert.deepEqual(segments("abc", []), [{ text: "abc", hit: false }]);
  assert.deepEqual(segments("", [[0, 0]]), []);
  const runs = segments("abcdef", [[2, 2]]);
  assert.deepEqual(runs, [
    { text: "ab", hit: false },
    { text: "c", hit: true },
    { text: "def", hit: false },
  ]);
  assert.equal(runs.map((run) => run.text).join(""), "abcdef");
});

test("pattern hints keep the order the footer shows", () => {
  assert.deepEqual(PATTERN_HINTS, ["//...", ":all", ":*"]);
});
