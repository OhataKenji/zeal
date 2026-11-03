import Anser from "anser";
import { LOG_LINE_HEIGHT, TERMINAL_COLORS } from "./terminal_style.ts";

const ANSI_COLORS: Record<string, string> = Object.fromEntries(
  Object.entries(TERMINAL_COLORS).map(([name, value]) => [
    `ansi-${name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`,
    value,
  ]),
);

function color(name: string, truecolor: string): string | undefined {
  if (!name) return;
  if (ANSI_COLORS[name]) return ANSI_COLORS[name];
  if (name === "ansi-truecolor") return `rgb(${truecolor})`;
  const index = Number(name.match(/^ansi-palette-(\d+)$/)?.[1]);
  if (index >= 232 && index <= 255) {
    const gray = 8 + (index - 232) * 10;
    return `rgb(${gray}, ${gray}, ${gray})`;
  }
  if (index >= 16 && index < 232) {
    const levels = [0, 95, 135, 175, 215, 255];
    const n = index - 16;
    return `rgb(${levels[Math.floor(n / 36)]}, ${levels[Math.floor(n / 6) % 6]}, ${levels[n % 6]})`;
  }
}

/** One display line of styled log text, ready for row-at-a-time rendering. */
export type LogLine = Array<{ text: string; style: string }>;

/** Splits parsed log spans into display lines; `\r` and `\n` both break lines. */
export function logLines(log: string): LogLine[] {
  const lines: LogLine[] = [[]];
  const append = (text: string, style: string) => {
    if (text) lines[lines.length - 1].push({ text, style });
  };
  for (const span of logSpans(log)) {
    for (const [index, text] of span.text.split(/\r\n|\r|\n/).entries()) {
      if (index > 0) lines.push([]);
      append(text, span.style);
    }
  }
  return lines;
}

/** Conservative width in monospace columns: tabs use 8, non-ASCII UTF-16 units use 2. */
export function logColumns(lines: LogLine[]): number {
  let widest = 0;
  for (const line of lines) {
    let columns = 0;
    for (const span of line) {
      for (const char of span.text) {
        columns += char === "\t" ? 8 : char.charCodeAt(0) > 0x7f ? char.length * 2 : 1;
      }
    }
    widest = Math.max(widest, columns);
  }
  return widest;
}

/** Extra lines mounted above and below the viewport to hide scroll popping. */
const OVERSCAN = 8;

/**
 * First and last line index to mount, clamped so a shrinking log cannot
 * strand the viewport behind an oversized top spacer.
 */
export function logWindow(
  count: number,
  scrollTop: number,
  viewHeight: number,
): { first: number; last: number } {
  const first = Math.min(Math.max(0, Math.floor(scrollTop / LOG_LINE_HEIGHT) - OVERSCAN), count);
  const last = Math.min(count, first + Math.ceil(viewHeight / LOG_LINE_HEIGHT) + 2 * OVERSCAN + 1);
  return { first, last };
}

/** Parse styles only. Content stays text so log output can never become HTML. */
export function logSpans(log: string): Array<{ text: string; style: string }> {
  return Anser.ansiToJson(log, { remove_empty: true, use_classes: true }).map((entry) => {
    const styles: string[] = [];
    const fg = color(entry.fg, entry.fg_truecolor);
    const bg = color(entry.bg, entry.bg_truecolor);
    if (fg) styles.push(`color:${fg}`);
    if (bg) styles.push(`background-color:${bg}`);
    if (entry.decorations.includes("bold")) styles.push("font-weight:700");
    if (entry.decorations.includes("dim")) styles.push("opacity:0.5");
    if (entry.decorations.includes("italic")) styles.push("font-style:italic");
    const lines = [];
    if (entry.decorations.includes("underline")) lines.push("underline");
    if (entry.decorations.includes("strikethrough")) lines.push("line-through");
    if (lines.length) styles.push(`text-decoration:${lines.join(" ")}`);
    return { text: entry.content, style: styles.join(";") };
  });
}
