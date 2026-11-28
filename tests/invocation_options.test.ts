import { test } from "node:test";
import assert from "node:assert/strict";
import {
  formatOptions,
  invocationTargetFlags,
  quoteShellArgument,
} from "../src/lib/invocation/options.ts";

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

test("invocationTargetFlags keeps query-safe build flags and drops BES/query-reserved flags", () => {
  assert.equal(
    invocationTargetFlags([
      "--config=asan",
      "-k",
      "-c",
      "opt",
      "--nocache_test_results",
      "--keep_going",
      "--bes_backend=grpc://127.0.0.1:4718",
      "--invocation_id=abc",
      "--script_path=/tmp/run.sh",
      "--output=proto",
      "--profile=/tmp/p.gz",
      "--jobs=8",
      "--define=msg=hello world",
      "--",
      "--ignored",
    ]),
    "--config=asan --keep_going --compilation_mode=opt --keep_going --define='msg=hello world'",
  );
  assert.equal(invocationTargetFlags(undefined), "");
});
