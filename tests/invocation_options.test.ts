import { test } from "node:test";
import assert from "node:assert/strict";
import {
  formatOptions,
  parseExplicitOptions,
  quoteShellArgument,
  rerunOptions,
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

test("rerunOptions strips injected flags while preserving user options and overrides", () => {
  assert.deepEqual(
    rerunOptions([
      "--bes_backend=grpc://127.0.0.1:4718",
      "--bes_proxy=",
      "--bes_upload_mode=wait_for_upload_complete",
      "--invocation_id=old-id",
      "--target_pattern_file=targets.txt",
      "--tool_tag=zelith",
      "--color=yes",
      "--curses=no",
      "--config=asan",
      "--test_env=FOO=hello world",
    ]),
    ["--config=asan", "--test_env=FOO=hello world"],
  );
  assert.deepEqual(
    rerunOptions([
      "--invocation_id",
      "old-id",
      "--target_pattern_file",
      "targets.txt",
      "--tool_tag",
      "zelith",
      "--color=no",
      "--tool_tag=my-ci",
      "--target_pattern_file=",
      "--",
      "--invocation_id=app-arg",
    ]),
    ["--color=no", "--tool_tag=my-ci", "--target_pattern_file=", "--", "--invocation_id=app-arg"],
  );
  assert.deepEqual(rerunOptions(undefined), []);
});

test("parseExplicitOptions respects bareFlags and splits trailing residual args at --", () => {
  const bare = new Set(["--keep_going"]);
  const parsed = parseExplicitOptions(
    ["--keep_going", "-c", "opt", "--config=asan", "--", "--arg"],
    bare,
  );
  assert.deepEqual(parsed, {
    entries: [
      { name: "--keep_going", tokens: ["--keep_going"] },
      { name: "-c", value: "opt", tokens: ["-c", "opt"] },
      { name: "--config", value: "asan", tokens: ["--config=asan"] },
    ],
    residual: ["--", "--arg"],
  });
});
