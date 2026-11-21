import type { BazelCommand, BazelConfig } from "./types.ts";

/**
 * Launch affordances a `--config=<name>` definition supports, derived from
 * the Bazel command inheritance rules (`always`/`common` apply everywhere).
 */
export function configActions(config: BazelConfig): {
  canCquery: boolean;
  runCmd: BazelCommand | null;
} {
  const scopes = new Set(config.entries.map((entry) => entry.command));
  const build = ["always", "common", "build"].some((command) => scopes.has(command));
  const runCmd = build ? "build" : scopes.has("test") ? "test" : scopes.has("run") ? "run" : null;
  return {
    runCmd,
    canCquery: runCmd === "build" || runCmd === "test" || scopes.has("cquery"),
  };
}
