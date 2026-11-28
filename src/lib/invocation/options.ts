const SAFE_SHELL_ARGUMENT = /^[A-Za-z0-9_:\-,.%@/=]+$/;
const FLAG_ASSIGNMENT = /^--[A-Za-z0-9_:\-,.%@/]+=/;

// Transport and identity flags injected by Zelith (`src-tauri/src/runargs.rs`,
// `src-tauri/src/runner.rs`) that must not carry over to a re-run.
const STRIPPED_FLAGS = new Set([
  "--invocation_id",
  "--bes_backend",
  "--bes_proxy",
  "--bes_upload_mode",
]);

const STRIPPED_EXACT = new Map<string, string>([
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

function isStripped(name: string, value: string): boolean {
  if (name === "--target_pattern_file") return value !== "";
  return STRIPPED_FLAGS.has(name) || STRIPPED_EXACT.get(name) === value;
}

function skipCount(options: readonly string[], index: number): number {
  const arg = options[index];
  const eq = arg.indexOf("=");
  if (eq !== -1) return isStripped(arg.slice(0, eq), arg.slice(eq + 1)) ? 1 : 0;
  const next = options[index + 1];
  if (next === undefined) return STRIPPED_FLAGS.has(arg) ? 1 : 0;
  return isStripped(arg, next) ? 2 : 0;
}

/** Filters Zelith-injected transport/identity flags from BEP `explicitCmdLine`. */
export function rerunOptions(options: readonly string[] | undefined): string[] {
  if (!options?.length) return [];
  const kept: string[] = [];
  for (let i = 0; i < options.length; i += 1) {
    if (options[i] === "--") return [...kept, ...options.slice(i)];
    const skip = skipCount(options, i);
    if (skip > 0) i += skip - 1;
    else kept.push(options[i]);
  }
  return kept;
}
