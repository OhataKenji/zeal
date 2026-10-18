import Fuse from "fuse.js";
import type { Snapshot } from "./types.ts";

export interface Candidate {
  kind: "target" | "file";
  /** The key Fuse matches on: the full label (targets) or workspace-relative path (files). */
  key: string;
  /** The text shown in the row: the label itself, or a file's basename. */
  text: string;
  /** The string inserted into the target field when the row is picked. */
  insert: string;
  /** Right-hand annotation: the rule class, or `→ <owner label>` for files. */
  note: string;
}

export interface Match {
  candidate: Candidate;
  /** Highlight ranges into `candidate.text`, inclusive [start, end] pairs. */
  indices: readonly (readonly [number, number])[];
}

/**
 * Rows to search: every snapshot target in order, then each input file once as
 * a route to its owner. Outputs are not candidates; they are not sendable.
 */
export function candidates(snap: Snapshot): Candidate[] {
  const out: Candidate[] = snap.targets.map((target) => ({
    kind: "target",
    key: target.label,
    text: target.label,
    insert: target.label,
    note: target.ruleClass,
  }));

  const ownersByPath = new Map<string, Set<string>>();
  for (const file of snap.files) {
    if (file.role === "out") continue;
    let owners = ownersByPath.get(file.path);
    if (!owners) ownersByPath.set(file.path, (owners = new Set()));
    owners.add(file.owner);
  }
  for (const [path, owners] of ownersByPath) {
    // Same rule as the M1 tree: several owners collapse into the label that
    // sorts first.
    const owner = [...owners].sort()[0];
    out.push({
      kind: "file",
      key: path,
      text: baseName(path),
      insert: owner,
      note: `→ ${owner}`,
    });
  }
  return out;
}

/**
 * Builds the index over `cands`. Every `app-state` event carries a new snapshot
 * object, so the component derives a fresh index on each state change — a
 * handful per run — rather than once per fetch; typing then only searches the
 * index it holds.
 */
export function createIndex(cands: Candidate[]): Fuse<Candidate> {
  return new Fuse(cands, {
    keys: ["key", "text"],
    ignoreLocation: true,
    includeMatches: true,
    threshold: 0.4,
    minMatchCharLength: 1,
  });
}

/**
 * Fuse decides matching, scoring and matched positions. `ignoreLocation` keeps
 * the tail of a long label searchable, and the matches drive the highlight.
 */
export function search(index: Fuse<Candidate>, query: string, limit = 20): Match[] {
  if (query === "") return [];
  return index
    .search(query)
    .slice(0, Math.max(0, limit))
    .map((result) => {
      const { item } = result;
      const onText = result.matches?.find((match) => match.key === "text");
      const onKey = result.matches?.find((match) => match.key === "key");
      // A file matched only through its path has no range inside the displayed
      // basename, so it shows without an underline.
      const ranges = onText ?? (item.text === item.key ? onKey : undefined);
      return { candidate: item, indices: ranges?.indices ?? [] };
    });
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
      const { candidate } = matches[moveIndex(state.index, 0, length)];
      return { kind: "pick", value: candidate.insert };
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

function baseName(path: string): string {
  return path.split("/").pop() ?? path;
}
