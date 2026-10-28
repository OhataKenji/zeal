// The composer accepts exactly one label or pattern, passed to bazel as one
// argument without trimming, splitting, quoting or escaping. This mirrors the
// core's validation; shell metacharacters in a single token stay valid because
// they are never interpreted.
// `\p{White_Space}` rather than `\s`: JS `\s` also matches U+FEFF, which the
// core accepts (`char::is_whitespace` is White_Space and `is_control` is Cc).
const FORBIDDEN_CHARACTERS = /[\p{White_Space}\p{Cc}]/u;

/** Validates a single target token without trimming or splitting it. */
export function targetError(raw: string): string | null {
  if (raw === "") return "Enter a target";
  if (raw.startsWith("-")) return 'A target cannot start with "-"';
  if (FORBIDDEN_CHARACTERS.test(raw)) {
    return "A target cannot contain whitespace or control characters";
  }
  return null;
}
