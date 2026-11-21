import type { BazelCommand, BazelConfig } from "./types.ts";

/** Command prefixes whose entries also apply to `cquery`. */
const CQUERY_COMMANDS = ["always", "common", "build", "test", "cquery"];

/**
 * Initial Run dialog command per matched scope, strongest first: `always`,
 * `common`, and `build` entries all run under `build`; otherwise `test`, then
 * `run`. Configs limited to `query`/`cquery`/`aquery` get no run command.
 */
const RUN_SCOPES: ReadonlyArray<readonly [BazelCommand, readonly string[]]> = [
  ["build", ["always", "common", "build"]],
  ["test", ["test"]],
  ["run", ["run"]],
];

/**
 * Launch affordances a `--config=<name>` definition supports, derived from
 * the Bazel command inheritance rules (`always`/`common` apply everywhere).
 */
export function configActions(config: BazelConfig): {
  canCquery: boolean;
  runCmd: BazelCommand | null;
} {
  const commands = new Set(config.entries.map((entry) => entry.command));
  const canCquery = CQUERY_COMMANDS.some((command) => commands.has(command));
  const runCmd =
    RUN_SCOPES.find(([, scope]) => scope.some((command) => commands.has(command)))?.[0] ?? null;
  return { canCquery, runCmd };
}
