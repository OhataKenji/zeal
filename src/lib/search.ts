import fuzzysort, { type SnapshotKey } from "fuzzysort";
import type { Snapshot, Target } from "./types.ts";

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

/**
 * Candidates for query target-pattern inputs. The snapshot's `targets` only
 * hold in-workspace rules (`kind("rule", //...)`), but queries also name
 * external repos — `@maven//`, `@pip//`, toolchains. Those arrive as `to` of
 * the workspace's own dependency edges (implicit deps are filtered at
 * ingest), so they join the index as label-only targets; internal edge
 * destinations would just duplicate `targets` and are skipped.
 */
export function querySearchTargets(snap: Snapshot): Target[] {
  const external = new Set<string>();
  for (const e of snap.edges) {
    if (e.to.startsWith("@")) external.add(e.to);
  }
  return [
    ...snap.targets,
    ...[...external].map((label): Target => ({
      label,
      pkg: "",
      name: "",
      ruleClass: "external",
      location: null,
    })),
  ];
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

/**
 * The runs of `text` to render, alternating between plain and highlighted.
 * Overlapping or adjacent ranges merge into one run.
 */
export function segments(
  text: string,
  indices: ReadonlyArray<readonly [number, number]>,
): Array<{ text: string; hit: boolean }> {
  const ranges = indices
    .map(([start, end]): readonly [number, number] => [
      Math.max(0, start),
      Math.min(text.length - 1, end),
    ])
    .filter(([start, end]) => start <= end)
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);

  const merged: Array<[number, number]> = [];
  for (const [start, end] of ranges) {
    const last = merged.at(-1);
    if (last && start <= last[1] + 1) {
      last[1] = Math.max(last[1], end);
    } else {
      merged.push([start, end]);
    }
  }

  const runs: Array<{ text: string; hit: boolean }> = [];
  let at = 0;
  for (const [start, end] of merged) {
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

/**
 * Translates fuzzy-finder chords into arrow keys before `keyAction` sees them:
 * Ctrl-p/k move up, Ctrl-n/j move down. Callers must `preventDefault` whenever
 * the result differs from `event.key` — on macOS the native Ctrl-k binding is
 * kill-to-end-of-line and Ctrl-j inserts a newline, both of which wipe the
 * field mid-typing.
 */
export function navKey(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey">,
): string {
  if (!event.ctrlKey || event.metaKey || event.altKey) return event.key;
  if (event.key === "k" || event.key === "p") return "ArrowUp";
  if (event.key === "j" || event.key === "n") return "ArrowDown";
  return event.key;
}

/** Keyboard actions returned to the dropdown for its current match list. */
export type KeyAction =
  | { kind: "move"; index: number }
  | { kind: "pick"; value: string }
  | { kind: "close" }
  | { kind: "none" };

/** Maps arrows, Enter, and Escape to dropdown actions; it never starts a run. */
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

/** Bazel target patterns shown as examples below the search results. */
export const PATTERN_HINTS: readonly string[] = ["//...", ":all", ":*"];

// The input's ARIA attributes and the dropdown markup are one contract split
// across two components, so both ids are built here.
/** Stable target listbox ID used by the search input's ARIA relationship. */
export const LIST_ID = "qo-list";

/** Returns a DOM ID for the search option at the given result index. */
export function optionId(index: number): string {
  return `qo-opt-${index}`;
}
