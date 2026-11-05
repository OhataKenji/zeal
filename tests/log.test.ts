import { test } from "node:test";
import assert from "node:assert/strict";
import { logColumns, logLines, logSpans, logWindow } from "../src/lib/log.ts";
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

test("logLines splits styled spans at LF, CR and CRLF boundaries", () => {
  const lines = logLines("\x1b[32mone\ntwo\x1b[0m\rthree\r\nfour");
  assert.deepEqual(
    lines.map((line) => line.map((span) => span.text).join("")),
    ["one", "two", "three", "four"],
  );
  // ANSI styling carries across the split until it is reset.
  assert.ok(lines[1][0].style.includes(`color:${TERMINAL_COLORS.green}`));
  assert.equal(lines[2][0].style, "");
});

test("logLines keeps blank lines and a trailing newline's empty row", () => {
  const lines = logLines("a\n\nb\n");
  assert.deepEqual(
    lines.map((line) => line.map((span) => span.text).join("")),
    ["a", "", "b", ""],
  );
  assert.deepEqual(logLines(""), [[]]);
});

test("logColumns reserves the widest line across ANSI style boundaries", () => {
  assert.equal(logColumns(logLines("short\n\x1b[31mlong \x1b[0mline\nend")), 9);
  assert.equal(logColumns(logLines("")), 0);
  assert.equal(logColumns([]), 0);
});

test("logColumns leaves conservative room for tabs and non-ASCII text", () => {
  assert.equal(logColumns(logLines("a\t界é🙂")), 17);
});

test("logWindow mounts only the viewport plus an overscan skirt", () => {
  // A 150px viewport shows 10 rows at 15px each.
  assert.deepEqual(logWindow(1000, 0, 150), { first: 0, last: 27 });
  assert.deepEqual(logWindow(1000, 15 * 500, 150), { first: 492, last: 519 });
  assert.equal(logWindow(1000, 15 * 999, 150).last, 1000);
});

test("logWindow clamps a stale scroll offset when the log shrinks", () => {
  assert.deepEqual(logWindow(10, 15 * 999, 150), { first: 10, last: 10 });
  assert.deepEqual(logWindow(0, 0, 150), { first: 0, last: 0 });
  assert.deepEqual(logWindow(5, 0, 150), { first: 0, last: 5 });
});

test("logWindow bounds the initial long-log viewport", () => {
  // 240px viewport / 15px rows = 16 rows + 16 overscan + 1 = 33 mounted lines.
  // Spacers carry the unmounted rows: 0 above, (5000 - 33) * 15px below.
  assert.deepEqual(logWindow(5000, 0, 240), { first: 0, last: 33 });
});

test("HTML-looking output stays literal text without generated markup", () => {
  const text = '<img src=x onerror="alert(1)"> & <script>test</script>';
  assert.deepEqual(
    logSpans(`\x1b[31m${text}\x1b[0m`).map((span) => span.text),
    [text],
  );
});
