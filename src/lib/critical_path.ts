import type { CriticalPathStep, Target } from "./types";

const BAZEL_OUT_PREFIX = /^bazel-out\/[^/]+\/(bin|genfiles|testlogs)\//;
const BAZEL_LABEL_RE = /(?:@{1,2}[A-Za-z0-9_.+~-]*)?\/\/[A-Za-z0-9_./-]*:[A-Za-z0-9_./+=,@~-]+/;
const WORKSPACE_PATH_RE = /[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)+/;

/** Formats a microsecond duration compactly for the critical-path list. */
export function formatDuration(us: number): string {
  if (us < 1000) return `${Math.round(us)}µs`;
  if (us < 1_000_000) return `${(us / 1000).toFixed(us < 10_000 ? 1 : 0)}ms`;
  const s = us / 1_000_000;
  return `${s.toFixed(s < 10 ? 2 : 1)}s`;
}

/**
 * Resolves a workspace or `bazel-out/` artifact path to the target in the
 * deepest matching package, or `null` when no workspace target matches.
 */
export function targetForArtifactPath(path: string, targets: readonly Target[]): string | null {
  const out = BAZEL_OUT_PREFIX.exec(path);
  const norm = (out ? path.slice(out[0].length) : path).replace(/^\.\//, "");
  if (!norm || norm.startsWith("external/")) return null;
  const inPkg = targets
    .filter((t) => !t.pkg || norm.startsWith(`${t.pkg}/`))
    .map((t) => ({ t, rel: t.pkg ? norm.slice(t.pkg.length + 1) : norm }));
  const maxPkg = Math.max(-1, ...inPkg.map(({ t }) => t.pkg.length));
  const cands = inPkg.filter(({ t }) => t.pkg.length === maxPkg);
  const stem = (rel: string) => rel.replace(/(?:\.[^/]+)+$/, "");
  const match =
    cands.find(({ t, rel }) => rel === t.name) ??
    cands.find(
      ({ t, rel }) =>
        stem(rel) === t.name || (out?.[1] === "testlogs" && rel.startsWith(`${t.name}/`)),
    ) ??
    cands.find(({ t, rel }) => stem(rel) === `lib${t.name}`);
  return match?.t.label ?? null;
}

/** Resolves a critical-path step to a workspace target label, if any. */
export function stepTargetLabel(step: CriticalPathStep, targets: readonly Target[]): string | null {
  if (!targets.length) return null;
  const known = new Set(targets.map((t) => t.label));
  const resolveLabel = (raw: string | undefined): string | null => {
    if (!raw) return null;
    if (known.has(raw)) return raw;
    const stripped = raw.replace(/^@{1,2}(?=\/\/)/, "");
    return known.has(stripped) ? stripped : null;
  };
  return (
    resolveLabel(step.target) ??
    (step.out ? targetForArtifactPath(step.out, targets) : null) ??
    resolveLabel(BAZEL_LABEL_RE.exec(step.name)?.[0]) ??
    (() => {
      const match = WORKSPACE_PATH_RE.exec(step.name);
      return match ? targetForArtifactPath(match[0], targets) : null;
    })()
  );
}
