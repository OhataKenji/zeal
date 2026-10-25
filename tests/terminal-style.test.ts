import { test } from "node:test";
import assert from "node:assert/strict";
import { terminalColors, terminalTheme } from "../src/lib/terminal-style.ts";

// WCAG relative luminance.
function luminance(hex: string): number {
  const channels = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const linear = channels.map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

// Neutrals are excluded on purpose: white/brightWhite are already invisible on
// the white background — a pre-existing issue outside this palette's scope.
const normal = ["red", "green", "yellow", "blue", "magenta", "cyan"] as const;
const bright = [
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
] as const;

test("ANSI colors stay readable as text on the terminal background", () => {
  for (const name of normal) {
    const ratio = contrast(terminalColors[name], terminalTheme.background);
    assert.ok(ratio >= 4.5, `${name} ${terminalColors[name]} is ${ratio.toFixed(2)}:1`);
  }
  // Brights carry emphasis, not body text; the floor keeps them off pastel.
  for (const name of bright) {
    const ratio = contrast(terminalColors[name], terminalTheme.background);
    assert.ok(ratio >= 3.5, `${name} ${terminalColors[name]} is ${ratio.toFixed(2)}:1`);
  }
});
