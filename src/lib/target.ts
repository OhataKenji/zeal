// The composer accepts exactly one label or pattern, passed to bazel as one
// argument without trimming, splitting, quoting or escaping. This mirrors the
// core's validation; shell metacharacters in a single token stay valid because
// they are never interpreted.
// `\p{White_Space}` rather than `\s`: JS `\s` also matches U+FEFF, which the
// core accepts (`char::is_whitespace` is White_Space and `is_control` is Cc).
const forbidden = /[\p{White_Space}\p{Cc}]/u;

export function targetError(raw: string): string | null {
  if (raw === "") return "対象を入力してください";
  if (raw.startsWith("-")) return '対象を "-" で始めることはできません';
  if (forbidden.test(raw)) return "対象に空白や制御文字は使えません";
  return null;
}
