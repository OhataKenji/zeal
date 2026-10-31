import { test } from "node:test";
import assert from "node:assert/strict";
import vectors from "./target-labels.json" with { type: "json" };
import { targetError } from "../src/lib/target.ts";

// The vectors are shared with the core's `validate_target`: editing
// target-labels.json changes both sides' tests at once.
test("labels and patterns pass through verbatim", () => {
  for (const { target } of vectors.accepted) {
    assert.equal(targetError(target), null, JSON.stringify(target));
  }
});

test("empty, whitespace, control characters and a leading dash are rejected", () => {
  for (const { target } of vectors.rejected) {
    assert.notEqual(targetError(target), null, JSON.stringify(target));
  }
});

test("rejections explain the rule that was broken", () => {
  assert.equal(targetError(""), "Enter a target");
  assert.equal(targetError("//a:b "), "A target cannot contain whitespace or control characters");
  assert.equal(targetError("a\0b"), "A target cannot contain whitespace or control characters");
  assert.equal(targetError("-//a:b"), 'A target cannot start with "-"');
});
