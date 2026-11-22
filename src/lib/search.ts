import fuzzysort, { type SnapshotKey } from "fuzzysort";
import type { Target } from "./types.ts";

/** Candidate and fuzzy-match ranges for one target label. */
export interface Match {
  target: Target;
  /** Highlight ranges into `target.label`, inclusive [start, end] pairs. */
  indices: ReadonlyArray<readonly [number, number]>;
}

/** The searchable targets plus their label snapshot, built once per index. */
export interface SearchIndex {
  targets: readonly Target[];
  labels: SnapshotKey<Target>;
}

/**
 * Builds the index over `targets`. The session swaps the snapshot in on each
 * accepted refresh, so the component derives a fresh index once per fetch;
 * typing then only searches the index it holds.
 */
export function createIndex(targets: readonly Target[]): SearchIndex {
  return { targets, labels: fuzzysort.snapshot(targets, { key: "label" }) };
}

/** A label-only candidate, for indexes over strings that are not targets. */
export function labelTarget(label: string, ruleClass: string): Target {
  return { label, pkg: "", name: "", ruleClass, location: null };
}

/**
 * Parses `//pkg:needle` and `//:needle` structured queries. The first colon
 * splits the package scope from the target-name fragment.
 */
function structured(query: string): { pkg: string; name: string } | null {
  if (!query.startsWith("//")) return null;
  const colon = query.indexOf(":");
  if (colon < 0) return null;
  return { pkg: query.slice(2, colon), name: query.slice(colon + 1) };
}

/**
 * Structured `//pkg:needle` matching: the package must match exactly — root
 * `//:` selects only targets whose labels have no package — and `needle` is a
 * case-insensitive substring of the target name. Candidates sort by name;
 * an empty needle lists the whole package.
 */
function structuredMatches(index: SearchIndex, pkg: string, name: string, limit: number): Match[] {
  const needle = name.toLowerCase();
  const found = index.targets
    .filter((target) => {
      // labelTarget candidates carry no name; keep them out of label queries.
      if (target.name === "") return false;
      if (pkg === "") {
        if (!target.label.startsWith("//:")) return false;
      } else if (target.pkg !== pkg) {
        return false;
      }
      return needle === "" || target.name.toLowerCase().includes(needle);
    })
    .sort(
      (a, b) =>
        a.name.localeCompare(b.name) || (a.label < b.label ? -1 : a.label > b.label ? 1 : 0),
    )
    .slice(0, limit);
  return found.map((target) => {
    const prefix = target.label.length - target.name.length;
    const at = needle === "" ? -1 : target.name.toLowerCase().indexOf(needle);
    return {
      target,
      indices: at < 0 ? [] : [[prefix + at, prefix + at + needle.length - 1] as const],
    };
  });
}

/**
 * Ordinary fuzzy matching over labels. fuzzysort decides matching, scoring
 * and matched positions; queries match subsequences and space-separated terms.
 * Root-package targets whose names start with the needle get a score boost so
 * `//:format` outranks `//app:format` for "format".
 */
function fuzzyMatches(index: SearchIndex, query: string, limit: number): Match[] {
  const needle = query.toLowerCase();
  return fuzzysort
    .go(query, index.labels, {
      limit,
      threshold: 0,
      scoreFn: (result) => {
        const name = result.obj.name.toLowerCase();
        if (result.obj.pkg === "" && name.startsWith(needle)) {
          return (name === needle ? 0.95 : 0.8) + 0.05 * result.score;
        }
        return 0.75 * result.score;
      },
    })
    .map((result) => ({
      target: result.obj,
      indices: result.indexes.map((position): readonly [number, number] => [position, position]),
    }));
}

/**
 * Searches `index` for `query`: a `//pkg:fragment` query matches target names
 * inside one package, anything else fuzzy-matches whole labels.
 */
export function search(index: SearchIndex, query: string, limit = 20): Match[] {
  const trimmed = query.trim();
  if (trimmed === "" || limit <= 0) return [];
  const scoped = structured(trimmed);
  if (scoped !== null) return structuredMatches(index, scoped.pkg, scoped.name, limit);
  return fuzzyMatches(index, trimmed, limit);
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

/** Maps arrows, Enter, and Escape to dropdown actions; it never starts an invocation. */
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
