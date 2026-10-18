import { test } from "node:test";
import assert from "node:assert/strict";
import {
  LIST_ID,
  PATTERN_HINTS,
  candidates,
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
const index = createIndex(candidates(snap));

test("candidates keep targets first with their rule class and route files to one owner", () => {
  const cands = candidates(snap);
  assert.deepEqual(cands.slice(0, 3), [
    {
      kind: "target",
      key: "//app/lib:parser",
      text: "//app/lib:parser",
      insert: "//app/lib:parser",
      note: "java_library",
    },
    {
      kind: "target",
      key: "//app/lib:parser_test",
      text: "//app/lib:parser_test",
      insert: "//app/lib:parser_test",
      note: "java_test",
    },
    {
      kind: "target",
      key: "//very/deep/package:some_long_target_name",
      text: "//very/deep/package:some_long_target_name",
      insert: "//very/deep/package:some_long_target_name",
      note: "genrule",
    },
  ]);
  assert.deepEqual(cands.slice(3), [
    {
      kind: "file",
      key: "app/lib/Parser.java",
      text: "Parser.java",
      insert: "//app/lib:aparser",
      note: "→ //app/lib:aparser",
    },
    {
      kind: "file",
      key: "app/lib/Lexer.java",
      text: "Lexer.java",
      insert: "//app/lib:lexer",
      note: "→ //app/lib:lexer",
    },
  ]);
});

test("output files are not candidates", () => {
  const cands = candidates(snap);
  assert.equal(
    cands.some((c) => c.key === "app/lib/parser.jar"),
    false,
  );
  assert.equal(cands.length, 5);
});

test("a query at the tail of a long label matches and highlights the tail", () => {
  const found = search(index, "target_name");
  assert.equal(found.length > 0, true);
  const top = found[0];
  assert.equal(top.candidate.key, "//very/deep/package:some_long_target_name");
  const hit = segments(top.candidate.text, top.indices)
    .filter((run) => run.hit)
    .map((run) => run.text)
    .join("");
  assert.equal(hit, "target_name");
  assert.equal(top.candidate.text.endsWith(hit), true);
});

test("files insert their owner and highlight the displayed basename", () => {
  const found = search(index, "Lexer");
  const file = found.find((match) => match.candidate.kind === "file");
  assert.ok(file);
  assert.deepEqual(file.candidate, {
    kind: "file",
    key: "app/lib/Lexer.java",
    text: "Lexer.java",
    insert: "//app/lib:lexer",
    note: "→ //app/lib:lexer",
  });
  const hit = segments(file.candidate.text, file.indices)
    .filter((run) => run.hit)
    .map((run) => run.text)
    .join("");
  assert.equal(hit, "Lexer");
});

test("a match found only through the path has no highlight", () => {
  const files = search(index, "app/lib").filter((match) => match.candidate.kind === "file");
  assert.equal(files.length > 0, true);
  const parser = files.find((match) => match.candidate.key === "app/lib/Parser.java");
  assert.ok(parser);
  assert.deepEqual(parser.indices, []);
  assert.equal(parser.candidate.insert, "//app/lib:aparser");
});

test("an empty query searches nothing and the limit caps the rows", () => {
  assert.deepEqual(search(index, ""), []);
  assert.equal(search(index, "parser", 1).length, 1);
  assert.equal(search(index, "parser", 2).length, 2);
  assert.equal(search(index, "parser", 0).length, 0);
  assert.equal(search(index, "parser").length > 2, true);
});

test("an index keeps answering its own queries, and another array makes another index", () => {
  const first = search(index, "parser");
  assert.equal(first.length > 0, true);
  assert.deepEqual(search(index, "parser"), first);

  // Index creation reads the candidate array, so a different snapshot needs a
  // different index rather than a search of the one above.
  const other = createIndex(
    candidates({
      ...snap,
      targets: [
        {
          label: "//other:only_here",
          pkg: "other",
          name: "only_here",
          ruleClass: "genrule",
          location: null,
        },
      ],
      files: [],
    }),
  );
  assert.notEqual(other, index);
  assert.deepEqual(search(other, "parser"), []);
  assert.deepEqual(
    search(other, "only_here").map((match) => match.candidate.key),
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
    value: matches[0].candidate.insert,
  });
  assert.deepEqual(keyAction({ open: true, index: 1 }, "Enter", matches), {
    kind: "pick",
    value: matches[1].candidate.insert,
  });
  assert.deepEqual(keyAction({ open: true, index: 99 }, "Enter", matches), {
    kind: "pick",
    value: matches.at(-1)!.candidate.insert,
  });
});

test("Enter cannot execute after confirmation, after Escape or without candidates", () => {
  const matches = search(index, "parser");
  assert.deepEqual(keyAction({ open: false, index: 0 }, "Enter", matches), { kind: "none" });
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Enter", []), { kind: "none" });
  assert.deepEqual(keyAction({ open: false, index: 0 }, "Enter", []), { kind: "none" });
});

test("pattern input uses the same candidate confirmation rule", () => {
  const matches = search(index, "//app/...");
  assert.ok(matches.length > 0);
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Enter", matches), {
    kind: "pick",
    value: matches[0].candidate.insert,
  });
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
