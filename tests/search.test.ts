import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createIndex,
  keyAction,
  labelTarget,
  moveIndex,
  optionId,
  search,
  segments,
} from "../src/lib/search.ts";
import type { Target, TargetIndex } from "../src/lib/types.ts";
import { splitLabel } from "../src/lib/model.ts";

function target(label: string, ruleClass = "cc_library"): Target {
  return { label, ...splitLabel(label), ruleClass, location: null };
}

const SNAPSHOT: TargetIndex = {
  meta: { fetchedAt: 1, status: "ok" },
  targets: [
    target("//app/lib:parser", "java_library"),
    target("//app/lib:parser_test", "java_test"),
    target("//very/deep/package:some_long_target_name", "genrule"),
  ],
};

// The index belongs to whoever holds the candidates: built once here, searched
// by every case below.
const SEARCH_INDEX = createIndex(SNAPSHOT.targets);

function labels(index: ReturnType<typeof createIndex>, query: string, limit = 20): string[] {
  return search(index, query, limit).map((match) => match.target.label);
}

const GREETER = "//examples/cpp/helloworld:greeter_client";
const GREETER_INDEX = createIndex(
  [GREETER, "//other/cpp/helloworld:greeter_client"].map((label) => target(label, "cc_binary")),
);

// Root-package targets plus same-named decoys in other packages.
const ROOT_INDEX = createIndex([
  target("//:gazelle", "gazelle"),
  target("//:format", "format_multirun"),
  target("//app:format", "sh_binary"),
  target("//app2:other"),
  target("//app/lib:parser", "java_library"),
]);

for (const query of [
  "examplehellow",
  "example hellow",
  "example helloworld",
  "helloworld example",
  "exhw",
  "EXAMPLE HELLOWORLD",
]) {
  test(`target search matches all fragments in ${JSON.stringify(query)}`, () => {
    assert.deepEqual(labels(GREETER_INDEX, query), [GREETER]);
  });
}

test("subsequence matches retain the matching characters", () => {
  const match = search(GREETER_INDEX, "examplehellow")[0];
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
  const match = search(SEARCH_INDEX, "app/lib:parser").find(
    (match) => match.target.label === "//app/lib:parser",
  );
  assert.ok(match);
  assert.deepEqual(match.target, SNAPSHOT.targets[0]);
});

test("file paths are not search candidates", () => {
  assert.deepEqual(labels(SEARCH_INDEX, "Lexer.java"), []);
  assert.deepEqual(labels(SEARCH_INDEX, "Parser.java"), []);
  assert.deepEqual(labels(SEARCH_INDEX, "parser.jar"), []);
});

test("structured //pkg: queries match target names inside one package", () => {
  assert.deepEqual(labels(SEARCH_INDEX, "//app/lib:"), [
    "//app/lib:parser",
    "//app/lib:parser_test",
  ]);
  // The name fragment matches case-insensitively.
  assert.deepEqual(labels(SEARCH_INDEX, "//app/lib:PAR"), [
    "//app/lib:parser",
    "//app/lib:parser_test",
  ]);
});

test("structured queries require the exact package and never leak into children", () => {
  // //app: sees only //app's own targets — not //app/lib's or //app2's.
  assert.deepEqual(labels(ROOT_INDEX, "//app:"), ["//app:format"]);
  assert.deepEqual(labels(ROOT_INDEX, "//app/lib:"), ["//app/lib:parser"]);
  assert.deepEqual(labels(ROOT_INDEX, "//app2:"), ["//app2:other"]);
  // A package path used as the name fragment is not a target name.
  assert.deepEqual(labels(ROOT_INDEX, "//app:lib"), []);
  // The root scope //: lists only root-package targets, sorted by name.
  assert.deepEqual(labels(ROOT_INDEX, "//:"), ["//:format", "//:gazelle"]);
  assert.deepEqual(labels(ROOT_INDEX, "//:for"), ["//:format"]);
});

test("structured queries exclude label-only and external candidates", () => {
  const candidates = createIndex([
    labelTarget("//:fake", "alias"),
    labelTarget("//app:fake", "alias"),
    // An external label whose package parses as root still is not //:.
    target("@ext//:tool"),
    target("@ext//app:tool"),
    target("@@ext//app:tool"),
  ]);
  assert.deepEqual(labels(candidates, "//:"), []);
  assert.deepEqual(labels(candidates, "//app:"), []);
  assert.deepEqual(labels(candidates, "//:fake"), []);
  assert.deepEqual(labels(candidates, "//:tool"), []);
});

test("structured matches highlight the matched name at its label offset", () => {
  const match = search(SEARCH_INDEX, "//app/lib:par")[0];
  assert.ok(match);
  assert.equal(match.target.label, "//app/lib:parser");
  // "//app/lib:" is 10 characters, so "par" highlights label [10, 12].
  assert.deepEqual(match.indices, [[10, 12]]);
  const mid = search(SEARCH_INDEX, "//app/lib:ser").find(
    (candidate) => candidate.target.label === "//app/lib:parser",
  );
  assert.ok(mid);
  // "ser" is name offset 3 inside "parser" → label offset 13.
  assert.deepEqual(mid.indices, [[13, 15]]);
  const deep = search(SEARCH_INDEX, "//very/deep/package:long")[0];
  assert.ok(deep);
  assert.equal(deep.target.label, "//very/deep/package:some_long_target_name");
  // The name starts after "//very/deep/package:" (20 chars); "long" sits at
  // name offset 5, so the highlight lands on label [25, 28].
  assert.deepEqual(deep.indices, [[25, 28]]);
  // An empty needle lists the package without highlight ranges.
  const all = search(SEARCH_INDEX, "//app/lib:");
  assert.equal(all.length, 2);
  assert.ok(all.every((candidate) => candidate.indices.length === 0));
});

test("root-package name matches outrank other packages under a tight limit", () => {
  assert.deepEqual(labels(ROOT_INDEX, "format", 1), ["//:format"]);
  assert.deepEqual(labels(ROOT_INDEX, "for", 1), ["//:format"]);
  assert.deepEqual(labels(ROOT_INDEX, "gaz", 1), ["//:gazelle"]);
  // A root prefix match still beats a nested target whose name is exact.
  const prefixIndex = createIndex([
    target("//:formatter", "sh_binary"),
    target("//app:format", "sh_binary"),
    target("@ext//:format", "sh_binary"),
  ]);
  assert.deepEqual(labels(prefixIndex, "format", 1), ["//:formatter"]);
  // External labels stay searchable through ordinary fuzzy matching.
  assert.ok(labels(prefixIndex, "format").includes("@ext//:format"));
});

test("later searches do not change earlier results", () => {
  const first = search(GREETER_INDEX, "example");
  const saved = structuredClone(first);
  search(GREETER_INDEX, "greeter");
  assert.deepEqual(first, saved);
  assert.deepEqual(search(GREETER_INDEX, "example"), saved);
});

test("a query at the tail of a long label matches and highlights the tail", () => {
  const found = search(SEARCH_INDEX, "target_name");
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
  assert.deepEqual(search(SEARCH_INDEX, ""), []);
  assert.deepEqual(search(SEARCH_INDEX, "   "), []);
  assert.equal(search(SEARCH_INDEX, "parser", 1).length, 1);
  assert.equal(search(SEARCH_INDEX, "parser", 2).length, 2);
  assert.equal(search(SEARCH_INDEX, "parser", 0).length, 0);
  assert.deepEqual(search(SEARCH_INDEX, "parser", -1), []);
  assert.equal(search(SEARCH_INDEX, "parser").length, 2);
});

test("an index keeps answering its own queries, and another array makes another index", () => {
  const first = search(SEARCH_INDEX, "parser");
  assert.equal(first.length > 0, true);
  assert.deepEqual(search(SEARCH_INDEX, "parser"), first);

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
  assert.notEqual(other, SEARCH_INDEX);
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
  const matches = search(SEARCH_INDEX, "parser");
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
  const matches = search(SEARCH_INDEX, "parser");
  assert.ok(matches.length >= 2);
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Enter", matches), {
    kind: "pick",
    value: matches[0].target.label,
  });
  assert.deepEqual(keyAction({ open: true, index: 1 }, "Enter", matches), {
    kind: "pick",
    value: matches[1].target.label,
  });
  const lastMatch = matches.at(-1);
  assert.ok(lastMatch);
  assert.deepEqual(keyAction({ open: true, index: 99 }, "Enter", matches), {
    kind: "pick",
    value: lastMatch.target.label,
  });
});

test("Enter cannot execute after confirmation, after Escape or without candidates", () => {
  const matches = search(SEARCH_INDEX, "parser");
  assert.deepEqual(keyAction({ open: false, index: 0 }, "Enter", matches), { kind: "none" });
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Enter", []), { kind: "none" });
  assert.deepEqual(keyAction({ open: false, index: 0 }, "Enter", []), { kind: "none" });
});

test("patterns without matching candidates remain available for Send", () => {
  const matches = search(SEARCH_INDEX, "//app/...");
  assert.deepEqual(matches, []);
  assert.deepEqual(keyAction({ open: true, index: 0 }, "Enter", matches), { kind: "none" });
  // Closing suggestions leaves the typed pattern for the separate Send action.
  assert.deepEqual(keyAction({ open: false, index: 0 }, "Enter", matches), { kind: "none" });
});

test("Escape closes only an open dropdown and other keys do nothing", () => {
  const matches = search(SEARCH_INDEX, "parser");
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

test("option ids are scoped to their listbox id", () => {
  assert.equal(optionId("qo-list", 0), "qo-list-opt-0");
  assert.equal(optionId("qo-list", 12), "qo-list-opt-12");
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
