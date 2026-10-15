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
