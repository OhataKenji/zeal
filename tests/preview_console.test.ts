import { test } from "node:test";
import assert from "node:assert/strict";
import { interpretConsoleInput } from "../src/preview/console.ts";

test("preview console accumulates and echoes input across writes", () => {
  const first = interpretConsoleInput("", "1");
  assert.deepEqual(first, { input: "1", output: "1", exitCode: null });
  assert.deepEqual(interpretConsoleInput(first.input, "2"), {
    input: "12",
    output: "2",
    exitCode: null,
  });
});

test("preview console edits the input before submitting a number", () => {
  assert.deepEqual(interpretConsoleInput("12", "\x7f3\r\nignored"), {
    input: "13",
    output: "\b \b3\r\n26\r\n",
    exitCode: 0,
  });
});

test("preview console accepts a newline and doubles an empty input as zero", () => {
  assert.deepEqual(interpretConsoleInput("", "\n"), {
    input: "",
    output: "\r\n0\r\n",
    exitCode: 0,
  });
});

test("preview console echoes backspace even when input is empty", () => {
  assert.deepEqual(interpretConsoleInput("", "\x7f"), {
    input: "",
    output: "\b \b",
    exitCode: null,
  });
});

test("preview console interrupts on Ctrl-C without consuming following input", () => {
  assert.deepEqual(interpretConsoleInput("1", "2\x03ignored"), {
    input: "12",
    output: "2^C\r\n",
    exitCode: 130,
  });
});

test("preview console ends on Ctrl-D with the existing interrupted status", () => {
  assert.deepEqual(interpretConsoleInput("12", "\x04\n"), {
    input: "12",
    output: "^D\r\n",
    exitCode: 130,
  });
});
