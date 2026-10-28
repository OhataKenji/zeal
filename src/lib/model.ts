import type { FileEntry, Snapshot, Target } from "./types";

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

/** Returns the final slash-delimited segment of a path. */
export function basename(path: string): string {
  return path.split("/").pop() ?? path;
}

// ---------------------------------------------------------------------------
// Tree
//
// The directory hierarchy of packages is the skeleton, with rules inside each
// package. Files belong in the selected target's attributes, not in this tree.

/** Directory node used to group targets by Bazel package. */
export interface DirNode {
  path: string;
  name: string;
  dirs: DirNode[];
  rules: Target[];
}

/** Builds the package tree from target package paths. */
export function buildTree(snap: Snapshot): DirNode {
  const nodes = new Map<string, DirNode>();
  const mk = (path: string): DirNode => ({
    path,
    name: path === "" ? "" : basename(path),
    dirs: [],
    rules: [],
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

  for (const target of snap.targets) {
    ensure(target.pkg).rules.push(target);
  }

  const sortNode = (n: DirNode): void => {
    n.dirs.sort((a, b) => a.name.localeCompare(b.name));
    n.rules.sort((a, b) => a.name.localeCompare(b.name));
    n.dirs.forEach(sortNode);
  };
  sortNode(root);
  return root;
}

/** A visible directory or target row in the package tree. */
export interface Row {
  key: string;
  depth: number;
  kind: "dir" | "rule";
  text: string;
  /** For dir rows: the directory path that expand/collapse toggles. */
  dirPath?: string;
  expanded?: boolean;
  /** For rule rows: the label selecting this row navigates to. */
  label?: string;
  ruleClass?: string;
}

/** Flattens expanded directories into rows in display order. */
export function flatten(root: DirNode, expanded: Set<string>): Row[] {
  const rows: Row[] = [];
  const walk = (n: DirNode, depth: number) => {
    for (const d of n.dirs) {
      const ex = expanded.has(d.path);
      rows.push({
        key: `d:${d.path}`,
        depth,
        kind: "dir",
        text: `${d.name}/`,
        dirPath: d.path,
        expanded: ex,
      });
      if (ex) walk(d, depth + 1);
    }
    for (const t of n.rules) {
      rows.push({
        key: `r:${t.label}`,
        depth,
        kind: "rule",
        text: `:${t.name}`,
        ruleClass: t.ruleClass,
        label: t.label,
      });
    }
  };
  walk(root, 0);
  return rows;
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
// The four relationship lines

/** Targets and files that depend on the selected target. */
export interface UsedBy {
  label: string;
  attrs: string[];
}

/** Direct dependency, data, and reverse-usage relationships for a target. */
export interface Relations {
  target: Target;
  deps: string[];
  data: string[];
  usedBy: UsedBy[];
  inputs: FileEntry[];
  outputs: FileEntry[];
}

/** Returns direct target relationships, or null if the label is unknown. */
export function relations(snap: Snapshot, label: string): Relations | null {
  const target = snap.targets.find((t) => t.label === label);
  if (!target) return null;

  const deps = new Set<string>();
  const data = new Set<string>();
  const usedBy = new Map<string, Set<string>>();
  for (const e of snap.edges) {
    if (e.from === label) {
      if (e.attr === "deps") {
        deps.add(e.to);
      } else if (e.attr === "data") {
        data.add(e.to);
      }
    }
    if (e.to === label) {
      let attrs = usedBy.get(e.from);
      if (!attrs) usedBy.set(e.from, (attrs = new Set()));
      attrs.add(e.attr);
    }
  }

  return {
    target,
    deps: [...deps].sort(),
    data: [...data].sort(),
    usedBy: [...usedBy]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([label, attrs]) => ({ label, attrs: [...attrs].sort() })),
    inputs: snap.files.filter((f) => f.owner === label && f.role !== "out"),
    outputs: snap.files.filter((f) => f.owner === label && f.role === "out"),
  };
}
