//! The in-memory snapshot: the four structures the UI renders.
//!
//! - **targets**: label, package, name, rule class, BUILD location
//! - **edges**: directed dependency edges (from, to, attribute name)
//! - **files**: (workspace-relative path, owner label, role); role is the
//!   attribute name on the input side and `out` on the output side
//! - **meta**: fetch time and status

use crate::label;
use crate::xml::RawRule;
use serde::Serialize;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetLocation {
    /// Absolute path of the BUILD (or .bzl) file.
    pub file: String,
    pub line: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub label: String,
    pub pkg: String,
    pub name: String,
    pub rule_class: String,
    pub location: Option<TargetLocation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// Attribute the edge came through (`deps`, `data`, `tools`, ...).
    pub attr: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct FileEntry {
    /// Workspace-relative path.
    pub path: String,
    /// Label of the rule that references (input) or produces (output) it.
    pub owner: String,
    /// Attribute name on the input side (`srcs`, `data`, ...), `out` on the
    /// output side.
    pub role: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ok,
    /// exit != 0 but at least one rule was accepted (`--keep_going`).
    Partial,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// Milliseconds since the UNIX epoch.
    pub fetched_at: u64,
    pub status: Status,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub meta: Meta,
    pub targets: Vec<Target>,
    pub edges: Vec<Edge>,
    pub files: Vec<FileEntry>,
}

/// Build a snapshot from parsed rules. Labels are canonicalized before being
/// classified: a label that names a known target or starts with `@`/`@@`
/// becomes an edge; anything else is a file reference.
pub fn build(rules: &[RawRule], status: Status) -> Snapshot {
    let mut known = HashSet::new();
    let mut targets = Vec::new();
    for r in rules {
        let lbl = label::canonical(&r.label);
        if !known.insert(lbl.clone()) {
            continue;
        }
        let (pkg, name) = label::split(&lbl);
        targets.push(Target {
            label: lbl,
            pkg,
            name,
            rule_class: r.rule_class.clone(),
            location: r
                .location
                .clone()
                .map(|(file, line)| TargetLocation { file, line }),
        });
    }
    targets.sort_by(|a, b| a.label.cmp(&b.label));

    let mut edges = HashSet::new();
    let mut files = HashSet::new();
    for r in rules {
        let owner = label::canonical(&r.label);
        for (attr, raw) in &r.label_attrs {
            if !label::is_dep_attr(attr) {
                continue;
            }
            let to = label::canonical(raw);
            if label::is_select_key(&to) {
                continue;
            }
            if known.contains(&to) || label::is_external(&to) {
                edges.insert(Edge {
                    from: owner.clone(),
                    to,
                    attr: attr.clone(),
                });
            } else {
                files.insert(FileEntry {
                    path: label::label_to_path(&to),
                    owner: owner.clone(),
                    role: attr.clone(),
                });
            }
        }
        // Outputs come only from <rule-output>: output-typed attributes like
        // `outs` arrive as <output> elements and are never label_attrs.
        for out in &r.outputs {
            files.insert(FileEntry {
                path: label::label_to_path(&label::canonical(out)),
                owner: owner.clone(),
                role: "out".to_string(),
            });
        }
    }

    let mut edges: Vec<Edge> = edges.into_iter().collect();
    edges.sort_by(|a, b| (&a.from, &a.attr, &a.to).cmp(&(&b.from, &b.attr, &b.to)));
    let mut files: Vec<FileEntry> = files.into_iter().collect();
    files.sort_by(|a, b| (&a.path, &a.owner, &a.role).cmp(&(&b.path, &b.owner, &b.role)));

    Snapshot {
        meta: Meta {
            fetched_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            status,
        },
        targets,
        edges,
        files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(label: &str, class: &str, attrs: &[(&str, &str)], outputs: &[&str]) -> RawRule {
        RawRule {
            label: label.to_string(),
            rule_class: class.to_string(),
            location: None,
            label_attrs: attrs
                .iter()
                .map(|(a, v)| (a.to_string(), v.to_string()))
                .collect(),
            outputs: outputs.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn edge(from: &str, attr: &str, to: &str) -> Edge {
        Edge {
            from: from.to_string(),
            to: to.to_string(),
            attr: attr.to_string(),
        }
    }

    fn file(path: &str, owner: &str, role: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            owner: owner.to_string(),
            role: role.to_string(),
        }
    }

    fn sample_rules() -> Vec<RawRule> {
        let mut bin = rule(
            "//app:bin",
            "cc_binary",
            &[
                ("srcs", "//app:main.cc"),
                ("deps", "//app/lib"),
                ("deps", "//app/lib:lib"),
                ("deps", "@@zlib+//:z"),
                ("deps", "//conditions:default"),
                ("visibility", "//visibility:public"),
                ("$toolchain", "//tools:cc"),
            ],
            &["//app:bin"],
        );
        bin.location = Some(("/repo/app/BUILD".to_string(), 3));
        vec![
            bin,
            rule(
                "//app/lib:lib",
                "cc_library",
                &[("srcs", "//app/lib:lib.cc")],
                &[],
            ),
            // Same target as //app:bin after canonicalization.
            rule("@@//app:bin", "cc_binary", &[], &[]),
        ]
    }

    #[test]
    fn builds_sorted_deduplicated_targets() {
        let snap = build(&sample_rules(), Status::Ok);
        let labels: Vec<&str> = snap.targets.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, ["//app/lib:lib", "//app:bin"]);

        let bin = &snap.targets[1];
        assert_eq!((bin.pkg.as_str(), bin.name.as_str()), ("app", "bin"));
        assert_eq!(bin.rule_class, "cc_binary");
        let loc = bin.location.as_ref().unwrap();
        assert_eq!((loc.file.as_str(), loc.line), ("/repo/app/BUILD", 3));
        assert!(snap.targets[0].location.is_none());
    }

    #[test]
    fn classifies_label_attrs_into_edges_and_files() {
        let snap = build(&sample_rules(), Status::Ok);
        assert_eq!(
            snap.edges,
            [
                edge("//app:bin", "deps", "//app/lib:lib"),
                edge("//app:bin", "deps", "@zlib+//:z"),
            ]
        );
        assert_eq!(
            snap.files,
            [
                file("app/bin", "//app:bin", "out"),
                file("app/lib/lib.cc", "//app/lib:lib", "srcs"),
                file("app/main.cc", "//app:bin", "srcs"),
            ]
        );
    }

    #[test]
    fn records_status_and_fetch_time() {
        let ok = build(&sample_rules(), Status::Ok);
        assert!(matches!(ok.meta.status, Status::Ok));
        assert!(ok.meta.fetched_at > 0);

        let partial = build(&[], Status::Partial);
        assert!(matches!(partial.meta.status, Status::Partial));
        assert!(partial.targets.is_empty());
        assert!(partial.edges.is_empty());
        assert!(partial.files.is_empty());
    }
}
