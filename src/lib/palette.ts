import type { SnapshotKey } from "fuzzysort";
import { invocationStatus } from "./invocation.ts";
import { search } from "./search.ts";
import { pages, type Page } from "./pages.ts";
import type { InvocationRecord, OwnedInvocation, Target } from "./types.ts";

export type PaletteGroup = "commands" | "targets" | "invocations";

export type PaletteAction =
  | { readonly kind: "page"; readonly page: Page }
  | { readonly kind: "run" }
  | { readonly kind: "refresh" }
  | { readonly kind: "open-workspace" }
  | { readonly kind: "target"; readonly label: string }
  | { readonly kind: "invocation"; readonly id: string };

export interface PaletteItem {
  readonly id: string;
  readonly group: PaletteGroup;
  readonly label: string;
  readonly detail?: string;
  readonly shortcut?: string;
  readonly action: PaletteAction;
}

export interface PaletteInput {
  readonly query: string;
  readonly targetIndex: SnapshotKey<Target> | null;
  readonly invocations: ReadonlyArray<{
    readonly record: InvocationRecord;
    readonly execution?: OwnedInvocation;
  }>;
  readonly canRun: boolean;
  readonly canRefresh: boolean;
  readonly isMac: boolean;
}

const MATCH_LIMIT = 8;
const EMPTY_QUERY_INVOCATIONS = 5;
const QUERY_LIMIT = 512;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

interface PaletteCommand {
  readonly id: string;
  readonly label: string;
  readonly keywords: string;
  readonly shortcut?: string;
  readonly action: PaletteAction;
}

const pageKeywords: Record<Page, string> = {
  workspace: "home bazel info",
  targets: "explorer packages tree",
  query: "expr bazel",
  history: "invocations logs runs",
  analytics: "stats metrics",
};

function commands(input: PaletteInput): PaletteCommand[] {
  const list: PaletteCommand[] = pages.map(({ id, label }) => ({
    id: `command:page-${id}`,
    label: `Go to ${label}`,
    keywords: pageKeywords[id],
    action: { kind: "page", page: id },
  }));
  if (input.canRun) {
    list.push({
      id: "command:run",
      label: "Run Bazel…",
      keywords: "build test execute",
      shortcut: input.isMac ? "⌘⇧R" : "Ctrl+Shift+R",
      action: { kind: "run" },
    });
  }
  if (input.canRefresh) {
    list.push({
      id: "command:refresh",
      label: "Reload targets",
      keywords: "refresh info scope sync",
      action: { kind: "refresh" },
    });
  }
  list.push({
    id: "command:open-workspace",
    label: "Open workspace…",
    keywords: "folder choose switch",
    shortcut: input.isMac ? "⌘O" : "Ctrl+O",
    action: { kind: "open-workspace" },
  });
  return list;
}

function commandMatches(command: PaletteCommand, needle: string): boolean {
  return (
    command.label.toLowerCase().includes(needle) || command.keywords.toLowerCase().includes(needle)
  );
}

function invocationItem(row: {
  readonly record: InvocationRecord;
  readonly execution?: OwnedInvocation;
}): PaletteItem {
  const { record, execution } = row;
  const status = invocationStatus(record, execution);
  return {
    id: `invocation:${record.id}`,
    group: "invocations",
    label: `${record.cmd ?? "?"} ${record.pattern ?? record.id}`,
    detail: `${status.label} · ${record.id.slice(0, 8)}`,
    action: { kind: "invocation", id: record.id },
  };
}

function invocationMatches(row: { readonly record: InvocationRecord }, needle: string): boolean {
  return (
    row.record.id.toLowerCase().includes(needle) ||
    (row.record.pattern ?? "").toLowerCase().includes(needle) ||
    (row.record.cmd ?? "").toLowerCase().includes(needle)
  );
}

/** Recent rows for an empty query; capped, id-promoted matches otherwise. */
function invocationRows(
  input: PaletteInput,
  query: string,
  needle: string,
  uuid: string | null,
): PaletteInput["invocations"] {
  if (query === "") return input.invocations.slice(0, EMPTY_QUERY_INVOCATIONS);
  const hits = input.invocations.filter((row) => invocationMatches(row, needle));
  // An exact id match outranks rows that only contain the UUID inside a
  // pattern, and promoting it keeps the record inside the per-group cap.
  const exact =
    uuid === null ? undefined : hits.find((row) => row.record.id.toLowerCase() === uuid);
  return (exact === undefined ? hits : [exact, ...hits.filter((row) => row !== exact)]).slice(
    0,
    MATCH_LIMIT,
  );
}

/** Builds the flat, grouped result list for one palette query. */
export function paletteItems(input: PaletteInput): PaletteItem[] {
  const query = input.query.slice(0, QUERY_LIMIT).trim();
  const needle = query.toLowerCase();
  const uuid = UUID_PATTERN.test(query) ? needle : null;
  const available = commands(input);
  const commandItems: PaletteItem[] = (
    query === ""
      ? available
      : available.filter((command) => commandMatches(command, needle)).slice(0, MATCH_LIMIT)
  ).map((command) => ({
    id: command.id,
    group: "commands",
    label: command.label,
    shortcut: command.shortcut,
    action: command.action,
  }));

  const targetItems: PaletteItem[] =
    query === "" || input.targetIndex === null
      ? []
      : search(input.targetIndex, query, MATCH_LIMIT).map((match) => ({
          id: `target:${match.target.label}`,
          group: "targets",
          label: match.target.label,
          detail: match.target.ruleClass,
          action: { kind: "target", label: match.target.label },
        }));

  const invocationItems: PaletteItem[] = invocationRows(input, query, needle, uuid).map(
    invocationItem,
  );

  // An RFC 4122 UUID that history does not know yet becomes a direct lookup;
  // the session pins it through list_invocations(selectedId).
  if (uuid !== null && !input.invocations.some((row) => row.record.id.toLowerCase() === uuid)) {
    invocationItems.push({
      id: `invocation:${uuid}`,
      group: "invocations",
      label: uuid,
      detail: "Look up invocation ID",
      action: { kind: "invocation", id: uuid },
    });
  }

  return [...commandItems, ...targetItems, ...invocationItems];
}
