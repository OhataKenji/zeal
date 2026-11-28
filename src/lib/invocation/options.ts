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

export interface ParsedOption {
  name: string;
  value?: string;
  tokens: readonly string[];
}

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

function nextOption(
  options: readonly string[],
  index: number,
  bareFlags?: ReadonlySet<string>,
): { entry: ParsedOption; consumed: number } {
  const token = options[index];
  const eq = token.indexOf("=");
  if (eq !== -1) {
    return {
      entry: { name: token.slice(0, eq), value: token.slice(eq + 1), tokens: [token] },
      consumed: 1,
    };
  }
  const next = options[index + 1];
  if (!bareFlags?.has(token) && next !== undefined && !next.startsWith("--")) {
    return { entry: { name: token, value: next, tokens: [token, next] }, consumed: 2 };
  }
  return { entry: { name: token, tokens: [token] }, consumed: 1 };
}

/**
 * Tokenizes BEP `explicitCmdLine` up to `--` into `{ name, value, tokens }`
 * entries plus any trailing `residual` tokens starting at `--`.
 */
export function parseExplicitOptions(
  options: readonly string[] | undefined,
  bareFlags?: ReadonlySet<string>,
): { entries: ParsedOption[]; residual: readonly string[] } {
  if (!options?.length) return { entries: [], residual: [] };
  const entries: ParsedOption[] = [];
  for (let i = 0; i < options.length;) {
    if (options[i] === "--") return { entries, residual: options.slice(i) };
    const { entry, consumed } = nextOption(options, i, bareFlags);
    entries.push(entry);
    i += consumed;
  }
  return { entries, residual: [] };
}

function isStrippedRerunOption({ name, value }: ParsedOption): boolean {
  if (name === "--target_pattern_file") return value !== undefined && value !== "";
  if (STRIPPED_FLAGS.has(name)) return true;
  return value !== undefined && STRIPPED_EXACT.get(name) === value;
}

/** Filters Zelith-injected transport/identity flags from BEP `explicitCmdLine`. */
export function rerunOptions(options: readonly string[] | undefined): string[] {
  const { entries, residual } = parseExplicitOptions(options);
  return [
    ...entries.filter((entry) => !isStrippedRerunOption(entry)).flatMap((entry) => entry.tokens),
    ...residual,
  ];
}
