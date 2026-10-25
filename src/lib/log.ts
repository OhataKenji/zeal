import Anser from "anser";
import { terminalColors } from "./terminal-style.ts";

const colors: Record<string, string> = Object.fromEntries(
  Object.entries(terminalColors).map(([name, value]) => [
    `ansi-${name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`,
    value,
  ]),
);

function color(name: string, truecolor: string): string | undefined {
  if (!name) return;
  if (colors[name]) return colors[name];
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

/** Parse styles only. Content stays text so log output can never become HTML. */
export function logSpans(log: string): { text: string; style: string }[] {
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
