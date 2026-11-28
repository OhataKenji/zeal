import { test } from "node:test";
import assert from "node:assert/strict";
import { formatOptions, quoteShellArgument, rerunOptions } from "../src/lib/invocation/options.ts";

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

test("rerunOptions strips Zelith-injected transport and identity flags while keeping user options", () => {
  assert.deepEqual(
    rerunOptions([
      "--bes_backend=grpc://127.0.0.1:4718",
      "--bes_proxy=",
      "--bes_upload_mode=wait_for_upload_complete",
      "--invocation_id=11111111-2222-3333-4444-555555555555",
      "--target_pattern_file=targets.txt",
      "--tool_tag=zelith",
      "--color=yes",
      "--curses=no",
      "--config=asan",
      "--test_env=FOO=hello world",
      "--test_output=errors",
    ]),
    ["--config=asan", "--test_env=FOO=hello world", "--test_output=errors"],
  );
});

test("rerunOptions strips two-token Zelith flags and preserves user overrides and post-separator args", () => {
  assert.deepEqual(
    rerunOptions([
      "--invocation_id",
      "old-id",
      "--bes_backend",
      "grpc://127.0.0.1:12345",
      "--bes_proxy",
      "",
      "--bes_upload_mode",
      "wait_for_upload_complete",
      "--target_pattern_file",
      "targets.txt",
      "--tool_tag",
      "zelith",
      "--color",
      "yes",
      "--curses",
      "no",
      "--color=no",
      "--tool_tag=my-ci",
      "--target_pattern_file=",
      "--config=opt",
      "--",
      "--invocation_id=app-arg",
    ]),
    [
      "--color=no",
      "--tool_tag=my-ci",
      "--target_pattern_file=",
      "--config=opt",
      "--",
      "--invocation_id=app-arg",
    ],
  );
  assert.deepEqual(rerunOptions(undefined), []);
  assert.deepEqual(rerunOptions([]), []);
});
