import { test } from "node:test";
import assert from "node:assert/strict";
import { formatOptions, quoteShellArgument } from "../src/lib/run/options.ts";

test("quotes every argv token without changing its shell argument boundary", () => {
  assert.equal(
    formatOptions([
      "--jobs=8",
      "--define=message=hello world",
      "",
      "--define=message=it's ready",
      "--define=message=$HOME `id`; echo unsafe",
      "--define=message=first\nsecond",
    ]),
    "--jobs=8 --define='message=hello world' '' --define='message=it'\\''s ready' " +
      "--define='message=$HOME `id`; echo unsafe' --define='message=first\nsecond'",
  );
});

test("quotes shell metacharacters and leaves empty option lists empty", () => {
  assert.equal(quoteShellArgument("a;b $HOME `id`"), "'a;b $HOME `id`'");
  assert.equal(quoteShellArgument("--keep_going"), "--keep_going");
  assert.equal(formatOptions([]), "");
});
