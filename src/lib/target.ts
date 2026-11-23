// The composer accepts exactly one label or pattern, passed to bazel as one
// argument without trimming, splitting, quoting or escaping. This mirrors the
// core's validation; shell metacharacters in a single token stay valid because
// they are never interpreted.
// `\p{White_Space}` rather than `\s`: JS `\s` also matches U+FEFF, which the
// core accepts (`char::is_whitespace` is White_Space and `is_control` is Cc).
const FORBIDDEN_CHARACTERS = /[\p{White_Space}\p{Cc}]/u;

/** Explains Explorer's package wildcards without implying discovery limits Bazel. */
export function packageScopeNote(pattern: string): string | null {
  if (!pattern.startsWith("//")) return null;
  const [pkg, target] = pattern.split(":");
  const wildcard = target === "all" || target === "*" || target === "all-targets";
  if (pkg.endsWith("/...") || !wildcard) return null;
  return "Zelith only lists targets in your workspace scope. Bazel may build or test unlisted targets in this package.";
}

/** Validates a single target token without trimming or splitting it. */
export function targetError(raw: string): string | null {
  if (raw === "") return "Enter a target or pattern";
  if (raw.startsWith("-")) return 'A target or pattern cannot start with "-"';
  if (FORBIDDEN_CHARACTERS.test(raw)) {
    return "A target or pattern cannot contain whitespace or control characters";
  }
  return null;
}
