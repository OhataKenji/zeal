const SAFE_SHELL_ARGUMENT = /^[A-Za-z0-9_:\-,.%@/=]+$/;
const FLAG_ASSIGNMENT = /^--[A-Za-z0-9_:\-,.%@/]+=/;

// Transport, identity, and target-selector flags that must not carry over to a
// re-run (see `src-tauri/src/runargs.rs` and `src-tauri/src/runner.rs`).
const STRIPPED_VALUE_FLAGS = new Set([
  "--invocation_id",
  "--bes_backend",
  "--bes_proxy",
  "--bes_upload_mode",
  "--target_pattern_file",
]);

const STRIPPED_EXACT_FLAGS = new Map<string, string>([
  ["--tool_tag", "zelith"],
  ["--color", "yes"],
  ["--curses", "no"],
]);

/** Quotes one argument for a shell display without changing its value. */
export function quoteShellArgument(argument: string): string {
  if (SAFE_SHELL_ARGUMENT.test(argument)) return argument;

  const prefix = FLAG_ASSIGNMENT.exec(argument)?.[0] ?? "";
  const suffix = argument.slice(prefix.length).replaceAll("'", "'\\''");
  return `${prefix}'${suffix}'`;
}

/** Formats command options as a space-separated shell display. */
export function formatOptions(options: readonly string[]): string {
  return options.map(quoteShellArgument).join(" ");
}

function isStrippedOption(name: string, value: string): boolean {
  return STRIPPED_VALUE_FLAGS.has(name) || STRIPPED_EXACT_FLAGS.get(name) === value;
}

/** Returns how many tokens at `index` are a Zelith-injected option to strip (0, 1, or 2). */
function strippedOptionLength(options: readonly string[], index: number): number {
  const arg = options[index];
  const eq = arg.indexOf("=");
  if (eq !== -1) {
    return isStrippedOption(arg.slice(0, eq), arg.slice(eq + 1)) ? 1 : 0;
  }
  const next = options[index + 1];
  if (next === undefined) return STRIPPED_VALUE_FLAGS.has(arg) ? 1 : 0;
  return isStrippedOption(arg, next) ? 2 : 0;
}

/**
 * Removes Zelith-injected transport/identity flags from BEP `explicitCmdLine`
 * so a re-run gets a fresh invocation ID and live BES endpoint while keeping
 * every user-supplied option.
 */
export function rerunOptions(options: readonly string[] | undefined): string[] {
  if (!options?.length) return [];
  const kept: string[] = [];
  for (let i = 0; i < options.length; i += 1) {
    if (options[i] === "--") {
      kept.push(...options.slice(i));
      break;
    }
    const skip = strippedOptionLength(options, i);
    if (skip > 0) {
      i += skip - 1;
    } else {
      kept.push(options[i]);
    }
  }
  return kept;
}
