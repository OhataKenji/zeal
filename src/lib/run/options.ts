import type { CommandLinePart } from "../types.ts";

const SAFE_SHELL_ARGUMENT = /^[A-Za-z0-9_:\-,.%@/=]+$/;
const FLAG_ASSIGNMENT = /^--[A-Za-z0-9_:\-,.%@/]+=/;
const STRUCTURED_OPTION_PREFIX = /^--?[A-Za-z0-9_:\-,.%@/]+(?:=|[ \t]+)$/;

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

/** Formats recorded structured command-line parts without splitting option values. */
export function formatCommandLine(parts: readonly CommandLinePart[]): string {
  return parts
    .map((part) => {
      if (part.kind === "chunk") return quoteShellArgument(part.value);

      const prefix = part.combinedForm.slice(0, part.combinedForm.length - part.optionValue.length);
      return part.combinedForm.endsWith(part.optionValue) && STRUCTURED_OPTION_PREFIX.test(prefix)
        ? `${prefix}${quoteShellArgument(part.optionValue)}`
        : quoteShellArgument(part.combinedForm);
    })
    .join(" ");
}
