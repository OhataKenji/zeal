import { test } from "node:test";
import assert from "node:assert/strict";
import { logSpans } from "../src/lib/log.ts";
import { TERMINAL_COLORS } from "../src/lib/terminal_style.ts";

test("Bazel color labels keep their text, color and reset across lines", () => {
  const spans = logSpans("\x1b[1;31mERROR:\x1b[0m failed\n\x1b[32mINFO:\x1b[0m next\n");
  assert.equal(spans.map((span) => span.text).join(""), "ERROR: failed\nINFO: next\n");
  assert.ok(spans[0].style.includes(`color:${TERMINAL_COLORS.red}`));
  assert.match(spans[0].style, /font-weight:700/);
  assert.equal(spans[1].style, "");
  assert.notEqual(spans[0].style, spans[2].style);
});

test("normal, bright and indexed ANSI colors use the Console palette", () => {
  const spans = logSpans("\x1b[32mnormal\x1b[92mbright\x1b[38;5;2mindexed\x1b[104mbackground");
  assert.equal(spans[0].style, `color:${TERMINAL_COLORS.green}`);
  assert.equal(spans[1].style, `color:${TERMINAL_COLORS.brightGreen}`);
  assert.equal(spans[2].style, `color:${TERMINAL_COLORS.green}`);
  assert.equal(
    spans[3].style,
    `color:${TERMINAL_COLORS.green};background-color:${TERMINAL_COLORS.brightBlue}`,
  );
});

test("extended grayscale and truecolor backgrounds retain their specified colors", () => {
  const spans = logSpans("\x1b[38;5;232mblack\x1b[38;5;255mwhite\x1b[48;2;10;20;30mbackground");
  assert.equal(spans[0].style, "color:rgb(8, 8, 8)");
  assert.equal(spans[1].style, "color:rgb(238, 238, 238)");
  assert.equal(spans[2].style, "color:rgb(238, 238, 238);background-color:rgb(10, 20, 30)");
});

test("extended colors are rendered and a new snapshot does not inherit old styles", () => {
  const [red, custom] = logSpans("\x1b[38;5;196mred\x1b[38;2;10;20;30mcustom");
  assert.equal(red.style, "color:rgb(255, 0, 0)");
  assert.equal(custom.style, "color:rgb(10, 20, 30)");
  assert.deepEqual(logSpans("plain"), [{ text: "plain", style: "" }]);
});

test("HTML-looking output stays literal text without generated markup", () => {
  const text = '<img src=x onerror="alert(1)"> & <script>test</script>';
  assert.deepEqual(
    logSpans(`\x1b[31m${text}\x1b[0m`).map((span) => span.text),
    [text],
  );
});
