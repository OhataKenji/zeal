import { describe, expect, it } from "vitest";
import {
  ancestorsOf,
  basename,
  buildTree,
  displayLabel,
  displayPath,
  flatten,
  relations,
  splitLabel,
  type DirNode,
} from "./model";
import type { Edge, FileEntry, Snapshot, Target } from "./types";

function target(label: string, ruleClass = "genrule"): Target {
  const { pkg, name } = splitLabel(label);
  return { label, pkg, name, ruleClass, location: null };
}

function snap(targets: Target[], edges: Edge[] = [], files: FileEntry[] = []): Snapshot {
  return { meta: { fetchedAt: 0, status: "ok" }, targets, edges, files };
}

const edge = (from: string, to: string, attr: string): Edge => ({ from, to, attr });
const file = (path: string, owner: string, role: string): FileEntry => ({ path, owner, role });

/** Child directory by name, failing the test when it is missing. */
function dir(n: DirNode, name: string): DirNode {
  const d = n.dirs.find((x) => x.name === name);
  if (!d) throw new Error(`no dir ${name} under "${n.path}"`);
  return d;
}

describe("splitLabel", () => {
  it("splits a canonical label", () => {
    expect(splitLabel("//a/b:t")).toEqual({ pkg: "a/b", name: "t" });
  });

  it("returns an empty package for the root package", () => {
    expect(splitLabel("//:t")).toEqual({ pkg: "", name: "t" });
  });

  it("drops the repository of an external label", () => {
    expect(splitLabel("@r//p:t")).toEqual({ pkg: "p", name: "t" });
    expect(splitLabel("@r//:t")).toEqual({ pkg: "", name: "t" });
  });

  it("keeps everything after the first colon as the name", () => {
    expect(splitLabel("//a:x/y.txt")).toEqual({ pkg: "a", name: "x/y.txt" });
  });

  it("uses the package as the name when there is no colon", () => {
    expect(splitLabel("//a")).toEqual({ pkg: "a", name: "a" });
  });

  // Bazel (and label::split in src-tauri) name `//a/b` as `b`; this returns
  // `a/b`. Labels from the snapshot are canonical (`//a/b:b`), so the UI does
  // not hit this today.
  it.todo("names `//a/b` after its last package segment ({ pkg: 'a/b', name: 'b' })");
});

describe("displayLabel", () => {
  it("shortens a label in the current package", () => {
    expect(displayLabel("//a:t", "a")).toBe(":t");
    expect(displayLabel("//:t", "")).toBe(":t");
  });

  it("keeps a label in another package", () => {
    expect(displayLabel("//a:t", "b")).toBe("//a:t");
    expect(displayLabel("//a/b:t", "a")).toBe("//a/b:t");
    expect(displayLabel("//:t", "a")).toBe("//:t");
  });

  it("keeps an external label even when its package matches", () => {
    expect(displayLabel("@r//a:t", "a")).toBe("@r//a:t");
  });
});

describe("displayPath", () => {
  it("strips the package prefix", () => {
    expect(displayPath("a/b/c.txt", "a")).toBe("b/c.txt");
    expect(displayPath("a/b/c.txt", "a/b")).toBe("c.txt");
  });

  it("keeps the path outside the package", () => {
    expect(displayPath("ab/c.txt", "a")).toBe("ab/c.txt");
    expect(displayPath("b/c.txt", "a")).toBe("b/c.txt");
  });

  it("keeps the path for the root package", () => {
    expect(displayPath("a/c.txt", "")).toBe("a/c.txt");
  });
});

describe("basename", () => {
  it("returns the last path segment", () => {
    expect(basename("a/b/c.txt")).toBe("c.txt");
    expect(basename("c.txt")).toBe("c.txt");
    expect(basename("")).toBe("");
  });
});

describe("ancestorsOf", () => {
  it("lists every prefix from the top, including the package itself", () => {
    expect(ancestorsOf("a/b/c")).toEqual(["a", "a/b", "a/b/c"]);
    expect(ancestorsOf("a")).toEqual(["a"]);
  });

  it("is empty for the root package", () => {
    expect(ancestorsOf("")).toEqual([]);
  });
});

describe("buildTree", () => {
  it("returns an empty root for an empty snapshot", () => {
    expect(buildTree(snap([]))).toEqual({
      path: "",
      name: "",
      isPackage: false,
      dirs: [],
      rules: [],
      files: [],
    });
  });

  it("places a single package under its directory chain", () => {
    const t = target("//a/b:t");
    const root = buildTree(snap([t]));
    const a = dir(root, "a");
    const b = dir(a, "b");
    expect(root.isPackage).toBe(false);
    expect(a).toMatchObject({ path: "a", isPackage: false, rules: [] });
    expect(b).toMatchObject({ path: "a/b", isPackage: true, rules: [t], dirs: [] });
  });

  it("marks the root as a package when it has rules", () => {
    const t = target("//:t");
    const root = buildTree(snap([t]));
    expect(root.isPackage).toBe(true);
    expect(root.rules).toEqual([t]);
    expect(root.dirs).toEqual([]);
  });

  it("sorts dirs, rules and files by name", () => {
    const root = buildTree(
      snap(
        [target("//b:z"), target("//b:y"), target("//a:t")],
        [],
        [file("b/n.txt", "//b:z", "srcs"), file("b/m.txt", "//b:y", "srcs")],
      ),
    );
    expect(root.dirs.map((d) => d.name)).toEqual(["a", "b"]);
    const b = dir(root, "b");
    expect(b.rules.map((r) => r.name)).toEqual(["y", "z"]);
    expect(b.files.map((f) => f.name)).toEqual(["m.txt", "n.txt"]);
  });

  it("lists only input-side files, deduplicated with sorted owners", () => {
    const root = buildTree(
      snap(
        [target("//a:x"), target("//a:y")],
        [],
        [
          file("a/in.txt", "//a:y", "srcs"),
          file("a/in.txt", "//a:x", "data"),
          file("a/in.txt", "//a:x", "srcs"),
          file("a/out.txt", "//a:x", "out"),
        ],
      ),
    );
    expect(dir(root, "a").files).toEqual([
      { path: "a/in.txt", name: "in.txt", owners: ["//a:x", "//a:y"] },
    ]);
  });

  it("puts a file in the deepest package containing it", () => {
    const root = buildTree(
      snap(
        [target("//:r"), target("//a:t"), target("//a/b:t")],
        [],
        [
          file("a/b/c/deep.txt", "//a:t", "srcs"),
          file("a/x/mid.txt", "//a:t", "srcs"),
          file("top.txt", "//a:t", "srcs"),
        ],
      ),
    );
    const a = dir(root, "a");
    expect(dir(a, "b").files.map((f) => f.path)).toEqual(["a/b/c/deep.txt"]);
    expect(a.files.map((f) => f.path)).toEqual(["a/x/mid.txt"]);
    expect(root.files.map((f) => f.path)).toEqual(["top.txt"]);
    // No directory nodes are created for file-only directories.
    expect(a.dirs.map((d) => d.name)).toEqual(["b"]);
  });

  it("falls back to the first owner's package when no package contains the file", () => {
    const root = buildTree(
      snap(
        [target("//lib:z"), target("//app:a")],
        [],
        [file("third_party/x.h", "//lib:z", "hdrs"), file("third_party/x.h", "//app:a", "srcs")],
      ),
    );
    expect(root.dirs.map((d) => d.name)).toEqual(["app", "lib"]);
    expect(dir(root, "app").files.map((f) => f.path)).toEqual(["third_party/x.h"]);
    expect(dir(root, "lib").files).toEqual([]);
  });

  // Files from different sub-directories of one package keep only their
  // basename, so `a/x/f.txt` and `a/y/f.txt` both show as `f.txt`.
  it("names a file by its basename even when it sits in a sub-directory", () => {
    const root = buildTree(
      snap(
        [target("//a:t")],
        [],
        [file("a/y/f.txt", "//a:t", "srcs"), file("a/x/f.txt", "//a:t", "srcs")],
      ),
    );
    expect(dir(root, "a").files.map((f) => [f.name, f.path])).toEqual([
      ["f.txt", "a/y/f.txt"],
      ["f.txt", "a/x/f.txt"],
    ]);
  });
});

describe("flatten", () => {
  const root = buildTree(
    snap(
      [target("//:r"), target("//a:t"), target("//a/b:u")],
      [],
      [file("a/in.txt", "//a:t", "srcs"), file("a/in.txt", "//a/b:u", "srcs")],
    ),
  );

  it("returns no rows for an empty tree", () => {
    expect(flatten(buildTree(snap([])), new Set())).toEqual([]);
  });

  it("shows only top-level rows when nothing is expanded", () => {
    expect(flatten(root, new Set())).toEqual([
      {
        key: "d:a",
        depth: 0,
        kind: "dir",
        text: "a/",
        sub: "//a",
        dirPath: "a",
        expanded: false,
      },
      { key: "r://:r", depth: 0, kind: "rule", text: ":r", label: "//:r" },
    ]);
  });

  it("walks expanded dirs depth-first: dirs, then rules, then files", () => {
    const rows = flatten(root, new Set(["a", "a/b"]));
    expect(rows.map((r) => [r.key, r.depth])).toEqual([
      ["d:a", 0],
      ["d:a/b", 1],
      ["r://a/b:u", 2],
      ["r://a:t", 1],
      ["f:a/in.txt", 1],
      ["r://:r", 0],
    ]);
  });

  // Owners are sorted by code unit, so `//a/b:u` comes before `//a:t`.
  it("shows owners relative to the file's package and selects the first sorted owner", () => {
    const f = flatten(root, new Set(["a"])).find((r) => r.kind === "file");
    expect(f).toEqual({
      key: "f:a/in.txt",
      depth: 1,
      kind: "file",
      text: "in.txt",
      sub: "→ //a/b:u · :t",
      label: "//a/b:u",
    });
  });

  it("gives no package sub-label to a directory that is not a package", () => {
    const rows = flatten(buildTree(snap([target("//a/b:t")])), new Set());
    expect(rows).toEqual([
      {
        key: "d:a",
        depth: 0,
        kind: "dir",
        text: "a/",
        sub: undefined,
        dirPath: "a",
        expanded: false,
      },
    ]);
  });

  it("does not show children of a collapsed parent even if they are expanded", () => {
    const rows = flatten(root, new Set(["a/b"]));
    expect(rows.map((r) => r.key)).toEqual(["d:a", "r://:r"]);
  });
});

describe("relations", () => {
  const lib = target("//a:lib", "cc_library");
  const bin = target("//b:bin", "cc_binary");
  const test = target("//b:test", "cc_test");
  const s = snap(
    [lib, bin, test],
    [
      edge("//b:bin", "//a:lib", "deps"),
      edge("//b:bin", "@r//x:ext", "deps"),
      edge("//b:test", "//a:lib", "data"),
      edge("//b:test", "//a:lib", "deps"),
      edge("//b:test", "//b:bin", "deps"),
      edge("//b:test", "//b:bin", "data"),
      edge("//b:test", "//a:lib", "tools"),
    ],
    [
      file("a/lib.cc", "//a:lib", "srcs"),
      file("a/lib.h", "//a:lib", "hdrs"),
      file("a/liblib.a", "//a:lib", "out"),
      file("b/main.cc", "//b:bin", "srcs"),
    ],
  );

  it("returns null for an unknown label", () => {
    expect(relations(s, "//nope:x")).toBeNull();
    expect(relations(snap([]), "//a:lib")).toBeNull();
  });

  it("takes deps and data from outgoing edges, sorted", () => {
    const r = relations(s, "//b:test");
    expect(r?.target).toBe(test);
    expect(r?.deps).toEqual(["//a:lib", "//b:bin"]);
    expect(r?.data).toEqual(["//a:lib", "//b:bin"]);
  });

  it("keeps external labels in deps and ignores other attributes", () => {
    const r = relations(s, "//b:bin");
    expect(r?.deps).toEqual(["//a:lib", "@r//x:ext"]);
    expect(r?.data).toEqual([]);
    const other = relations(snap([lib, bin], [edge("//b:bin", "//a:lib", "tools")]), "//b:bin");
    expect(other?.deps).toEqual([]);
    expect(other?.data).toEqual([]);
  });

  it("deduplicates repeated outgoing edges", () => {
    const r = relations(
      snap([lib, bin], [edge("//b:bin", "//a:lib", "deps"), edge("//b:bin", "//a:lib", "deps")]),
      "//b:bin",
    );
    expect(r?.deps).toEqual(["//a:lib"]);
  });

  it("lists used-by from incoming edges of every attribute, sorted by label then attr", () => {
    expect(relations(s, "//a:lib")?.usedBy).toEqual([
      { label: "//b:bin", ruleClass: "cc_binary", attr: "deps" },
      { label: "//b:test", ruleClass: "cc_test", attr: "data" },
      { label: "//b:test", ruleClass: "cc_test", attr: "deps" },
      { label: "//b:test", ruleClass: "cc_test", attr: "tools" },
    ]);
    expect(relations(s, "//b:test")?.usedBy).toEqual([]);
  });

  it("leaves ruleClass empty when the user is not a known target", () => {
    const r = relations(snap([lib], [edge("//gone:x", "//a:lib", "deps")]), "//a:lib");
    expect(r?.usedBy).toEqual([{ label: "//gone:x", ruleClass: "", attr: "deps" }]);
  });

  it("splits the target's own files into inputs and outputs", () => {
    const r = relations(s, "//a:lib");
    expect(r?.inputs.map((f) => f.path)).toEqual(["a/lib.cc", "a/lib.h"]);
    expect(r?.outputs.map((f) => f.path)).toEqual(["a/liblib.a"]);
    expect(relations(s, "//b:test")?.inputs).toEqual([]);
    expect(relations(s, "//b:test")?.outputs).toEqual([]);
  });
});
