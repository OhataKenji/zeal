import fuzzysort, { type SnapshotKey } from "fuzzysort";
import type { Target } from "./types.ts";

/** Candidate and fuzzy-match ranges for one target label. */
export interface Match {
  target: Target;
  /** Highlight ranges into `target.label`, inclusive [start, end] pairs. */
  indices: ReadonlyArray<readonly [number, number]>;
}

/**
 * Builds the index over `targets`. The session swaps the snapshot in only when
 * its snapshotFetchedAt changes, so the component derives a fresh index once
 * per fetch;
 * typing then only searches the index it holds.
 */
export function createIndex(targets: Target[]): SnapshotKey<Target> {
  return fuzzysort.snapshot(targets, { key: "label" });
}

/** A label-only candidate, for indexes over strings that are not targets. */
export function labelTarget(label: string, ruleClass: string): Target {
  return { label, pkg: "", name: "", ruleClass, location: null };
}

/**
 * fuzzysort decides matching, scoring and matched positions. Queries match
 * subsequences and space-separated terms; the matches drive the highlight.
 */
export function search(index: SnapshotKey<Target>, query: string, limit = 20): Match[] {
  if (query.trim() === "" || limit <= 0) return [];
  return fuzzysort.go(query, index, { limit, threshold: 0 }).map((result) => ({
    target: result.obj,
    indices: result.indexes.map((position): readonly [number, number] => [position, position]),
  }));
}

/** Merges sorted ranges, combining overlapping or adjacent ones. */
function mergeRanges(ranges: ReadonlyArray<readonly [number, number]>): Array<[number, number]> {
  const merged: Array<[number, number]> = [];
  for (const [start, end] of ranges) {
    const last = merged.at(-1);
    if (last && start <= last[1] + 1) {
      last[1] = Math.max(last[1], end);
    } else {
      merged.push([start, end]);
    }
  }
  return merged;
}

/**
 * The runs of `text` to render, alternating between plain and highlighted.
 * Overlapping or adjacent ranges merge into one run.
 */
export function segments(
  text: string,
  indices: ReadonlyArray<readonly [number, number]>,
): Array<{ text: string; hit: boolean }> {
  const ranges = mergeRanges(
    indices
      .map(([start, end]): readonly [number, number] => [
        Math.max(0, start),
        Math.min(text.length - 1, end),
      ])
      .filter(([start, end]) => start <= end)
      .sort((a, b) => a[0] - b[0] || a[1] - b[1]),
  );

  const runs: Array<{ text: string; hit: boolean }> = [];
  let at = 0;
  for (const [start, end] of ranges) {
    if (start > at) runs.push({ text: text.slice(at, start), hit: false });
    runs.push({ text: text.slice(start, end + 1), hit: true });
    at = end + 1;
  }
  if (at < text.length) runs.push({ text: text.slice(at), hit: false });
  return runs;
}

/** Clamps a selected index within the available rows; an empty list returns 0. */
export function moveIndex(index: number, delta: number, length: number): number {
  if (length === 0) return 0;
  return Math.min(Math.max(index + delta, 0), length - 1);
}

/** Keyboard actions returned to the dropdown for its current match list. */
export type KeyAction =
  | { kind: "move"; index: number }
  | { kind: "pick"; value: string }
  | { kind: "close" }
  | { kind: "none" };

/**
 * The arrow-key action: an empty list swallows the key, a closed list reopens
 * at its first row, an open one moves the highlight by `delta`.
 */
function arrowAction(
  state: { open: boolean; index: number },
  delta: number,
  length: number,
): KeyAction {
  if (length === 0) return { kind: "none" };
  return state.open
    ? { kind: "move", index: moveIndex(state.index, delta, length) }
    : { kind: "move", index: 0 };
}

/** Maps arrows, Enter, and Escape to dropdown actions; it never starts a run. */
export function keyAction(
  state: { open: boolean; index: number },
  key: string,
  matches: Match[],
): KeyAction {
  const length = matches.length;
  switch (key) {
    case "ArrowDown":
      return arrowAction(state, 1, length);
    case "ArrowUp":
      return arrowAction(state, -1, length);
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

// The input's ARIA attributes and the dropdown markup are one contract split
// across two components, so option ids are built in one place. The listbox id
// is per combobox instance ($props.id()), so several inputs can coexist.
/** Returns a DOM ID for the search option at the given result index. */
export function optionId(listId: string, index: number): string {
  return `${listId}-opt-${index}`;
}
