import { test } from "node:test";
import assert from "node:assert/strict";
import { interpretConsoleInput } from "../src/preview/console.ts";

test("preview console echoes typed characters across writes", () => {
  const first = interpretConsoleInput("", "h");
  assert.deepEqual(first, { input: "h", output: "h", exitCode: null });
  assert.deepEqual(interpretConsoleInput(first.input, "i"), {
    input: "hi",
    output: "i",
    exitCode: null,
  });
});

test("preview console submits a line on Enter and keeps running", () => {
  assert.deepEqual(interpretConsoleInput("hi", "\r"), {
    input: "",
    output: "\r\n",
    exitCode: null,
  });
  assert.deepEqual(interpretConsoleInput("yo", "\n"), {
    input: "",
    output: "\r\n",
    exitCode: null,
  });
});

test("preview console treats CRLF as a single line ending", () => {
  assert.deepEqual(interpretConsoleInput("hi", "\r\nthere"), {
    input: "there",
    output: "\r\nthere",
    exitCode: null,
  });
});

test("preview console edits the line before submitting and continues", () => {
  assert.deepEqual(interpretConsoleInput("12", "\x7f3\r\nnext"), {
    input: "next",
    output: "\b \b3\r\nnext",
    exitCode: null,
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
