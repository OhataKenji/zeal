import { test } from "node:test";
import assert from "node:assert/strict";
import { targetError } from "../src/lib/target.ts";

test("labels and patterns pass through verbatim", () => {
  for (const raw of [
    "//app/lib:parser",
    "//...",
    ":all",
    ":*",
    "@maven//:guava",
    "//app/...",
    "//app:tar;get",
  ]) {
    assert.equal(targetError(raw), null, raw);
  }
});

test("empty, whitespace, control characters and a leading dash are rejected", () => {
  for (const raw of ["", " ", "//a:b\t", "a\nb", "a\0b", "-//a:b", "//a:b ", "\u3000"]) {
    assert.notEqual(targetError(raw), null, JSON.stringify(raw));
  }
});

test("the rejection set matches the core's Unicode White_Space + Cc rule", () => {
  // The core accepts U+FEFF (it is neither White_Space nor Cc), while JS `\s`
  // would reject it.
  assert.equal(targetError("//a:\uFEFFb"), null);
  // Other separators White_Space covers: NBSP, line/paragraph separator.
  for (const raw of ["//a:\u00A0b", "//a:\u2028b", "//a:\u2029b", "//a:\u0085b"]) {
    assert.notEqual(targetError(raw), null, JSON.stringify(raw));
  }
  // Cc: every control character, NUL included.
  for (const code of [0, 0x1f, 0x7f, 0x9f]) {
    const raw = `//a:${String.fromCharCode(code)}b`;
    assert.notEqual(targetError(raw), null, JSON.stringify(raw));
  }
});

test("rejections explain the rule that was broken", () => {
  assert.match(targetError("")!, /入力/);
  assert.match(targetError("//a:b ")!, /空白/);
  assert.match(targetError("a\0b")!, /空白/);
  assert.match(targetError("-//a:b")!, /"-"/);
});
