import { test } from "node:test";
import assert from "node:assert/strict";
import { formatCommandLine, formatOptions, quoteShellArgument } from "../src/lib/run/options.ts";

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

test("formats a complete command line in recorded order, preserving repeated flags and targets", () => {
  assert.equal(
    formatCommandLine([
      { kind: "chunk", value: "bazel" },
      { kind: "option", combinedForm: "--batch", optionValue: "true" },
      { kind: "chunk", value: "test" },
      { kind: "option", combinedForm: "--config=first", optionValue: "first" },
      { kind: "option", combinedForm: "--config=second", optionValue: "second" },
      { kind: "chunk", value: "//one:test" },
      { kind: "chunk", value: "//two:test" },
      { kind: "chunk", value: "--" },
      { kind: "chunk", value: "-//excluded:test" },
    ]),
    "bazel --batch test --config=first --config=second //one:test //two:test -- -//excluded:test",
  );
});

test("keeps option separators separate from values containing spaces or quotes", () => {
  assert.equal(
    formatCommandLine([
      {
        kind: "option",
        combinedForm: "--define=message=hello world",
        optionValue: "message=hello world",
      },
      {
        kind: "option",
        combinedForm: "--run_under echo it's ready",
        optionValue: "echo it's ready",
      },
      { kind: "option", combinedForm: "-c\topt", optionValue: "opt" },
      { kind: "option", combinedForm: "--define=", optionValue: "" },
    ]),
    "--define='message=hello world' --run_under 'echo it'\\''s ready' -c\topt --define=''",
  );
});

test("quotes raw argument boundaries and shell substitutions in structured options", () => {
  assert.equal(
    formatCommandLine([
      { kind: "chunk", value: "/path with space/bazel" },
      { kind: "chunk", value: "" },
      { kind: "chunk", value: "//pkg:$(id);`whoami`" },
      {
        kind: "option",
        combinedForm: "--define=key=$HOME $(id); `whoami`",
        optionValue: "key=$HOME $(id); `whoami`",
      },
      { kind: "option", combinedForm: "--define=first\nsecond", optionValue: "first\nsecond" },
    ]),
    "'/path with space/bazel' '' '//pkg:$(id);`whoami`' --define='key=$HOME $(id); `whoami`' --define='first\nsecond'",
  );
});

test("quotes malformed or unsafe option prefixes as whole arguments", () => {
  assert.equal(
    formatCommandLine([
      { kind: "option", combinedForm: "--foo=$(id)=value", optionValue: "value" },
      { kind: "option", combinedForm: "--foo;echo value", optionValue: "value" },
      { kind: "option", combinedForm: "--foo\nvalue", optionValue: "value" },
      { kind: "option", combinedForm: "--foo unusual value", optionValue: "different" },
    ]),
    "--foo='$(id)=value' '--foo;echo value' '--foo\nvalue' '--foo unusual value'",
  );
});

test("keeps a recorded empty command line empty", () => {
  assert.equal(formatCommandLine([]), "");
});
