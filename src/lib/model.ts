import type { FileEntry, Snapshot, Target } from "./types";

export function splitLabel(label: string): { pkg: string; name: string } {
  const i = label.indexOf("//");
  const rest = i >= 0 ? label.slice(i + 2) : label;
  const c = rest.indexOf(":");
  return c >= 0 ? { pkg: rest.slice(0, c), name: rest.slice(c + 1) } : { pkg: rest, name: rest };
}

/** `:name` when the label is in `curPkg`, else the full label. */
export function displayLabel(label: string, curPkg: string): string {
  if (!label.startsWith("//")) return label;
  const { pkg, name } = splitLabel(label);
  return pkg === curPkg ? `:${name}` : label;
}

/** A file path relative to `pkg` when it lives inside it. */
export function displayPath(path: string, pkg: string): string {
  return pkg !== "" && path.startsWith(pkg + "/") ? path.slice(pkg.length + 1) : path;
}

export function basename(path: string): string {
  return path.split("/").pop() ?? path;
}

// ---------------------------------------------------------------------------
// Tree
//
// The directory hierarchy of packages is the skeleton. Rules and input files
// are listed inside their package node; files that match no package collapse
// into their owner's package.

export interface TreeFile {
  path: string;
  name: string;
  owners: string[];
}

export interface DirNode {
  path: string;
  name: string;
  isPackage: boolean;
  dirs: DirNode[];
  rules: Target[];
  files: TreeFile[];
}

export function buildTree(snap: Snapshot): DirNode {
  const nodes = new Map<string, DirNode>();
  const mk = (path: string): DirNode => ({
    path,
    name: path === "" ? "" : basename(path),
    isPackage: false,
    dirs: [],
    rules: [],
    files: [],
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

  const packages = new Set(snap.targets.map((t) => t.pkg));
  for (const p of packages) ensure(p).isPackage = true;
  for (const t of snap.targets) ensure(t.pkg).rules.push(t);

  // Input-side files, deduplicated by path.
  const ownersByPath = new Map<string, Set<string>>();
  for (const f of snap.files) {
    if (f.role === "out") continue;
    let s = ownersByPath.get(f.path);
    if (!s) ownersByPath.set(f.path, (s = new Set()));
    s.add(f.owner);
  }
  for (const [path, owners] of ownersByPath) {
    const dir = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
    // The deepest package that is a prefix of the file's directory.
    const best = [...ancestorsOf(dir).reverse(), ""].find((p) => packages.has(p));
    const ownersAsc = [...owners].sort();
    const nodePath = best ?? splitLabel(ownersAsc[0]).pkg;
    ensure(nodePath).files.push({ path, name: basename(path), owners: ownersAsc });
  }

  const sortNode = (n: DirNode): void => {
    n.dirs.sort((a, b) => a.name.localeCompare(b.name));
    n.rules.sort((a, b) => a.name.localeCompare(b.name));
    n.files.sort((a, b) => a.name.localeCompare(b.name));
    n.dirs.forEach(sortNode);
  };
  sortNode(root);
  return root;
}

export interface Row {
  key: string;
  depth: number;
  kind: "dir" | "rule" | "file";
  text: string;
  sub?: string;
  /** For dir rows: the directory path that expand/collapse toggles. */
  dirPath?: string;
  expanded?: boolean;
  /** For rule/file rows: the label selecting this row navigates to. */
  label?: string;
}

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
        sub: d.isPackage ? `//${d.path}` : undefined,
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
        label: t.label,
      });
    }
    for (const f of n.files) {
      rows.push({
        key: `f:${f.path}`,
        depth,
        kind: "file",
        text: f.name,
        sub: `→ ${f.owners.map((o) => displayLabel(o, n.path)).join(" · ")}`,
        label: f.owners[0],
      });
    }
  };
  walk(root, 0);
  return rows;
}

/** All ancestor directory paths of a package (for expanding on selection). */
export function ancestorsOf(pkg: string): string[] {
  if (pkg === "") return [];
  const parts = pkg.split("/");
  return parts.map((_, i) => parts.slice(0, i + 1).join("/"));
}

// ---------------------------------------------------------------------------
// The four relationship lines

export interface UsedBy {
  label: string;
  ruleClass: string;
  attr: string;
}

export interface Relations {
  target: Target;
  deps: string[];
  data: string[];
  usedBy: UsedBy[];
  inputs: FileEntry[];
  outputs: FileEntry[];
}

export function relations(snap: Snapshot, label: string): Relations | null {
  const target = snap.targets.find((t) => t.label === label);
  if (!target) return null;
  const byLabel = new Map(snap.targets.map((t) => [t.label, t]));

  const deps = new Set<string>();
  const data = new Set<string>();
  const usedBy: UsedBy[] = [];
  for (const e of snap.edges) {
    if (e.from === label) {
      if (e.attr === "deps") deps.add(e.to);
      else if (e.attr === "data") data.add(e.to);
    }
    if (e.to === label) {
      usedBy.push({
        label: e.from,
        ruleClass: byLabel.get(e.from)?.ruleClass ?? "",
        attr: e.attr,
      });
    }
  }
  usedBy.sort((a, b) => a.label.localeCompare(b.label) || a.attr.localeCompare(b.attr));

  return {
    target,
    deps: [...deps].sort(),
    data: [...data].sort(),
    usedBy,
    inputs: snap.files.filter((f) => f.owner === label && f.role !== "out"),
    outputs: snap.files.filter((f) => f.owner === label && f.role === "out"),
  };
}
