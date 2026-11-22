import type { FileEntry, Target, TargetDetail, TargetIndex } from "./types";

/** Splits a Bazel label into package and target name. */
export function splitLabel(label: string): { pkg: string; name: string } {
  const i = label.indexOf("//");
  const rest = i >= 0 ? label.slice(i + 2) : label;
  const c = rest.indexOf(":");
  return c >= 0 ? { pkg: rest.slice(0, c), name: rest.slice(c + 1) } : { pkg: rest, name: rest };
}

/** Uses `:name` for the current package and the full label elsewhere. */
export function displayLabel(label: string, curPkg: string): string {
  if (!label.startsWith("//")) return label;
  const { pkg, name } = splitLabel(label);
  return pkg === curPkg ? `:${name}` : label;
}

/** Removes the package prefix when the file path lies inside that package. */
export function displayPath(path: string, pkg: string): string {
  return pkg !== "" && path.startsWith(pkg + "/") ? path.slice(pkg.length + 1) : path;
}

/** POSIX single-quote escaping; arguments of safe characters stay bare. */
export function shellQuote(argument: string): string {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(argument)) return argument;
  return `'${argument.replaceAll("'", `'\\''`)}'`;
}

/** An action's arguments joined into one shell-readable command line. */
export function commandLine(action: { arguments: string[] }): string {
  return action.arguments
    .map((argument, index) =>
      index === 0 ? `'${argument.replaceAll("'", `'\\''`)}'` : shellQuote(argument),
    )
    .join(" ");
}

/** Returns the final slash-delimited segment of a path. */
export function basename(path: string): string {
  return path.split("/").pop() ?? path;
}

/** Drops the `bazel-out/<config>/bin/` prefix so output paths read short. */
export function shortOutputPath(path: string): string {
  return path.replace(/^bazel-out\/[^/]+\/bin\//, "");
}

// ---------------------------------------------------------------------------
// Tree
//
// The directory hierarchy of packages is the skeleton; each package node also
// lists its direct targets as leaves, like files in a file tree. `count` is
// the total number of targets in the subtree. Files belong in the selected
// target's attributes, not in this tree.

/** Directory node used to group packages by Bazel package path. */
export interface DirNode {
  path: string;
  name: string;
  dirs: DirNode[];
  /** Targets declared in this package, sorted by name. */
  targets: Target[];
  /** Total targets in this subtree, local targets plus all descendants. */
  count: number;
}

/** Builds the package tree from target package paths. */
export function buildTree(index: TargetIndex): DirNode {
  const nodes = new Map<string, DirNode>();
  const mk = (path: string): DirNode => ({
    path,
    name: path === "" ? "" : basename(path),
    dirs: [],
    targets: [],
    count: 0,
  });
  const root = mk("");
  nodes.set("", root);
  const ensure = (path: string): DirNode => {
    let n = nodes.get(path);
    if (n) return n;
    n = mk(path);
    nodes.set(path, n);
    const parent = ensure(path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "");
    parent.dirs.push(n);
    return n;
  };

  for (const target of index.targets) {
    ensure(target.pkg).targets.push(target);
  }
  // One bottom-up pass sorts each level and rolls child counts upward.
  const finish = (n: DirNode): number => {
    n.dirs.sort((a, b) => a.name.localeCompare(b.name));
    n.targets.sort((a, b) => a.name.localeCompare(b.name));
    let count = n.targets.length;
    for (const child of n.dirs) count += finish(child);
    n.count = count;
    return count;
  };
  finish(root);
  return root;
}

/** One visible tree row: a directory with its expansion state, or a target leaf. */
export type Row =
  | { kind: "dir"; node: DirNode; depth: number; expanded: boolean }
  | { kind: "target"; target: Target; depth: number };

/**
 * Flattens the tree into display rows. Each level lists child directories
 * first — recursing into the expanded ones — then the node's own targets.
 */
export function flatten(root: DirNode, expanded: Set<string>): Row[] {
  const rows: Row[] = [];
  const visit = (node: DirNode, depth: number) => {
    for (const dir of node.dirs) {
      const open = expanded.has(dir.path);
      rows.push({ kind: "dir", node: dir, depth, expanded: open });
      if (open) visit(dir, depth + 1);
    }
    for (const target of node.targets) {
      rows.push({ kind: "target", target, depth });
    }
  };
  visit(root, 0);
  return rows;
}

/** Whether a rule class marks a test: `*_test` rules and `test_suite`s. */
export function isTestRule(ruleClass: string): boolean {
  return ruleClass.endsWith("_test") || ruleClass === "test_suite";
}

/** Returns package ancestors from the root through the selected package. */
export function ancestorsOf(pkg: string): string[] {
  if (pkg === "") return [];
  const parts = pkg.split("/");
  const ancestors: string[] = [];
  for (let index = 0; index < parts.length; index++) {
    ancestors.push(parts.slice(0, index + 1).join("/"));
  }
  return ancestors;
}

// ---------------------------------------------------------------------------
// Selected target attributes and outputs

/** Direct attributes and outputs for a target. */
export interface Relations {
  target: Target;
  attributes: Array<{ name: string; labels: string[]; files: string[] }>;
  outputs: FileEntry[];
}

/** Returns direct relationships from the selected target's detail. */
export function relations(detail: TargetDetail): Relations {
  const { target } = detail;
  const label = target.label;
  const attributes = new Map<string, { labels: Set<string>; files: Set<string> }>();
  function attribute(name: string) {
    let value = attributes.get(name);
    if (!value) {
      value = { labels: new Set(), files: new Set() };
      attributes.set(name, value);
    }
    return value;
  }
  for (const name of ["srcs", "data", "deps"]) attribute(name);
  for (const e of detail.edges) {
    if (e.from === label) {
      attribute(e.attr).labels.add(e.to);
    }
  }
  for (const file of detail.files) {
    if (file.owner === label && file.role !== "out") {
      attribute(file.role).files.add(file.path);
    }
  }

  return {
    target,
    attributes: [...attributes].map(([name, value]) => ({
      name,
      labels: [...value.labels].sort(),
      files: [...value.files].sort(),
    })),
    outputs: detail.files.filter((f) => f.owner === label && f.role === "out"),
  };
}
