import fuzzysort, { type SnapshotKey } from "fuzzysort";
import type { Target } from "./types.ts";

export interface Match {
  target: Target;
  /** Highlight ranges into `target.label`, inclusive [start, end] pairs. */
  indices: readonly (readonly [number, number])[];
}

/**
 * Builds the index over `targets`. Every `app-state` event carries a new snapshot
 * object, so the component derives a fresh index on each state change — a
 * handful per run — rather than once per fetch; typing then only searches the
 * index it holds.
 */
export function createIndex(targets: Target[]): SnapshotKey<Target> {
  return fuzzysort.snapshot(targets, { key: "label" });
}

/**
 * fuzzysort decides matching, scoring and matched positions. Queries match
 * subsequences and space-separated terms; the matches drive the highlight.
 */
export function search(index: SnapshotKey<Target>, query: string, limit = 20): Match[] {
  if (query.trim() === "" || limit <= 0) return [];
  return fuzzysort.go(query, index, { limit, threshold: 0 }).map((result) => ({
    target: result.obj,
    indices: result.indexes.map((position) => [position, position] as const),
  }));
}

/**
 * The runs of `text` to render, alternating between plain and highlighted.
 * Overlapping or adjacent ranges merge into one run.
 */
export function segments(
  text: string,
  indices: readonly (readonly [number, number])[],
): { text: string; hit: boolean }[] {
  const ranges = indices
    .map(([start, end]) => [Math.max(0, start), Math.min(text.length - 1, end)] as const)
    .filter(([start, end]) => start <= end)
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);

  const merged: [number, number][] = [];
  for (const [start, end] of ranges) {
    const last = merged.at(-1);
    if (last && start <= last[1] + 1) last[1] = Math.max(last[1], end);
    else merged.push([start, end]);
  }

  const runs: { text: string; hit: boolean }[] = [];
  let at = 0;
  for (const [start, end] of merged) {
    if (start > at) runs.push({ text: text.slice(at, start), hit: false });
    runs.push({ text: text.slice(start, end + 1), hit: true });
    at = end + 1;
  }
  if (at < text.length) runs.push({ text: text.slice(at), hit: false });
  return runs;
}

/** Selection stays inside the list, and an empty list has one place to be. */
export function moveIndex(index: number, delta: number, length: number): number {
  if (length === 0) return 0;
  return Math.min(Math.max(index + delta, 0), length - 1);
}

export type KeyAction =
  | { kind: "move"; index: number }
  | { kind: "pick"; value: string }
  | { kind: "close" }
  | { kind: "none" };

/**
 * Arrows move the selection and Enter picks the highlighted row. Running a
 * label or pattern is a separate action on the Send button.
 */
export function keyAction(
  state: { open: boolean; index: number },
  key: string,
  matches: Match[],
): KeyAction {
  const length = matches.length;
  switch (key) {
    case "ArrowDown":
      if (length === 0) return { kind: "none" };
      return state.open
        ? { kind: "move", index: moveIndex(state.index, 1, length) }
        : { kind: "move", index: 0 };
    case "ArrowUp":
      if (length === 0) return { kind: "none" };
      return state.open
        ? { kind: "move", index: moveIndex(state.index, -1, length) }
        : { kind: "move", index: 0 };
    case "Enter": {
      if (!state.open || length === 0) return { kind: "none" };
      const { target } = matches[moveIndex(state.index, 0, length)];
      return { kind: "pick", value: target.label };
    }
    case "Escape":
      return state.open ? { kind: "close" } : { kind: "none" };
    default:
      return { kind: "none" };
  }
}

export const PATTERN_HINTS: readonly string[] = ["//...", ":all", ":*"];

// The input's ARIA attributes and the dropdown markup are one contract split
// across two components, so both ids are built here.
export const LIST_ID = "qo-list";

export function optionId(index: number): string {
  return `qo-opt-${index}`;
}
