const SAFE_SHELL_ARGUMENT = /^[A-Za-z0-9_:\-,.%@/=]+$/;
const FLAG_ASSIGNMENT = /^--[A-Za-z0-9_:\-,.%@/]+=/;

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
