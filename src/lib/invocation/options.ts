const SAFE_SHELL_ARGUMENT = /^[A-Za-z0-9_:\-,.%@/=]+$/;
const FLAG_ASSIGNMENT = /^--[A-Za-z0-9_:\-,.%@/]+=/;
const QUERY_RESERVED_FLAGS = new Set(
  "--output --output_file --starlark:file --starlark:expr --query_file --include_commandline --noinclude_commandline --include_artifacts --noinclude_artifacts --transitions --notransitions --proto:include_configurations --noproto:include_configurations --profile --jobs --invocation_id --script_path".split(
    " ",
  ),
);
const ALLOWED_BARE_FLAGS = new Set(
  "--keep_going --nokeep_going --verbose_failures --noremote_accept_cached --noremote_upload_local_results".split(
    " ",
  ),
);
const INVOCATION_ONLY_RE = /^--(?:bes_|build_event_|experimental_build_event_)/;

/** Quotes one argument for a shell display without changing its value. */
export function quoteShellArgument(argument: string): string {
  if (SAFE_SHELL_ARGUMENT.test(argument)) return argument;

  const prefix = FLAG_ASSIGNMENT.exec(argument)?.[0] ?? "";
  const suffix = argument.slice(prefix.length).replaceAll("'", "'\\''");
  return `${prefix}'${suffix}'`;
}

/** Formats command options as a space-separated shell display. */
export function formatOptions(options: string[]): string {
  return options.map(quoteShellArgument).join(" ");
}

function readOption(
  options: readonly string[],
  index: number,
): { name: string; value?: string; nextIndex: number } {
  const token = options[index];
  const eq = token.indexOf("=");
  if (eq !== -1) {
    return { name: token.slice(0, eq), value: token.slice(eq + 1), nextIndex: index + 1 };
  }
  const next = options[index + 1];
  const bare = ALLOWED_BARE_FLAGS.has(token) || token.startsWith("--no");
  if (!bare && next !== undefined && !next.startsWith("-")) {
    return { name: token, value: next, nextIndex: index + 2 };
  }
  return { name: token, nextIndex: index + 1 };
}

function isQueryAllowedLongFlag(name: string): boolean {
  return (
    name.startsWith("--") &&
    name.length >= 3 &&
    !QUERY_RESERVED_FLAGS.has(name) &&
    !INVOCATION_ONLY_RE.test(name)
  );
}

function toTargetFlag(name: string, value: string | undefined): string | null {
  if (name === "-c" && value !== undefined) return `--compilation_mode=${value}`;
  if (name === "-k" && value === undefined) return "--keep_going";
  if (!isQueryAllowedLongFlag(name)) return null;
  if (value !== undefined) return `${name}=${value}`;
  if (ALLOWED_BARE_FLAGS.has(name)) return name;
  return null;
}

/**
 * Extracts build flags from BEP `explicitCmdLine` safe to pass into
 * `cquery`/`aquery` when inspecting a target from an invocation.
 */
export function invocationTargetFlags(options: readonly string[] | undefined): string {
  if (!options?.length) return "";
  const kept: string[] = [];
  for (let i = 0; i < options.length && options[i] !== "--";) {
    const { name, value, nextIndex } = readOption(options, i);
    const flag = toTargetFlag(name, value);
    if (flag) kept.push(flag);
    i = nextIndex;
  }
  return formatOptions(kept);
}
