//! The in-memory snapshot: the four structures the UI renders.
//!
//! - **targets**: label, package, name, rule class, BUILD location
//! - **edges**: directed dependency edges (from, to, attribute name)
//! - **files**: (workspace-relative path, owner label, role); role is the
//!   attribute name on the input side and `out` on the output side
//! - **meta**: fetch time and status

use crate::label;
use crate::label::LabelParts;
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
#[tracing::instrument(target = "zeal::operations", name = "snapshot.build", skip_all)]
#[must_use]
pub fn build(rules: &[RawRule], status: Status) -> Snapshot {
    let mut known = HashSet::new();
    let mut targets = Vec::new();
    for r in rules {
        let lbl = label::canonical(&r.label);
        if !known.insert(lbl.clone()) {
            continue;
        }
        let LabelParts { package, name } = label::split(&lbl);
        targets.push(Target {
            label: lbl,
            pkg: package,
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
    use crate::xml;
    use std::collections::HashMap;
    use std::io::Cursor;

    fn fixture_rules() -> Vec<xml::RawRule> {
        let xml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/query.xml"
        ))
        .unwrap();
        xml::parse_rules(Cursor::new(xml))
    }

    #[test]
    fn snapshot_targets_canonicalize_bare_labels() {
        let snap = build(&fixture_rules(), Status::Ok);
        let labels: HashSet<&str> = snap.targets.iter().map(|t| t.label.as_str()).collect();
        // `//app` canonicalizes to `//app:app`.
        assert!(labels.contains("//app:app"));
        assert!(!labels.contains("//app"));
    }

    #[test]
    fn snapshot_edges_only_from_dep_attrs() {
        let snap = build(&fixture_rules(), Status::Ok);
        let edges: HashSet<(&str, &str, &str)> = snap
            .edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str(), e.attr.as_str()))
            .collect();

        assert!(edges.contains(&("//app/lib:parser", "//app/lib:parser_proto", "deps")));
        // External-repo label: edge exists, canonical `@@` collapses to `@`.
        assert!(edges.contains(&(
            "//app/lib:parser",
            "@maven+//:com_google_guava_guava",
            "deps"
        )));
        // `data` pointing at a rule is an edge, not a file.
        assert!(edges.contains(&("//app/lib:parser", "//app:grammars", "data")));
        // Non-deps/data attrs still become edges (used by shows the attr name).
        assert!(edges.contains(&("//app:app", "//tools:launcher", "tools")));
        assert!(edges.contains(&("//app/lib:parser_test", "//app/lib:parser", "deps")));

        // Excluded: visibility, implicit `$` attrs, select() branch keys.
        assert!(!edges.iter().any(|e| e.1 == "//visibility:public"));
        assert!(!edges.iter().any(|e| e.2 == "$cc_toolchain"));
        assert!(!edges.iter().any(|e| e.1.contains("conditions")));
        // And rule-input never produces edges.
        assert!(!edges.iter().any(|e| e.1.contains("SHOULD_NOT")));
    }

    #[test]
    fn snapshot_files_inputs_outputs_and_multi_owner() {
        let snap = build(&fixture_rules(), Status::Ok);
        let mut owners: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        for f in &snap.files {
            owners
                .entry(f.path.as_str())
                .or_default()
                .push((f.owner.as_str(), f.role.as_str()));
        }

        // File referenced by two rules' srcs has both owners.
        let mut pj = owners["app/lib/Parser.java"].clone();
        pj.sort();
        assert_eq!(
            pj,
            [
                ("//app/lib:parser", "srcs"),
                ("//app/lib:parser_test", "srcs"),
            ]
        );

        // `data` label to a file lands in files with role `data`.
        assert_eq!(
            owners["app/lib/grammars.txt"],
            [("//app/lib:parser", "data")]
        );
        assert_eq!(owners["app/server.cfg"], [("//app:app", "data")]);

        // Outputs carry role `out`; a genrule's `outs` label merges with its
        // <rule-output> into a single entry.
        assert_eq!(
            owners["app/lib/libparser.jar"],
            [("//app/lib:parser", "out")]
        );
        assert_eq!(owners["tools/launcher.sh"], [("//tools:launcher", "out")]);

        // rule-input alone never creates a file entry.
        assert!(!owners.contains_key("tools/BUILD"));

        // Files are unique on (path, owner, role).
        let uniq: HashSet<(&str, &str, &str)> = snap
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.owner.as_str(), f.role.as_str()))
            .collect();
        assert_eq!(uniq.len(), snap.files.len());
    }

    #[test]
    fn edges_dedup() {
        let rules = xml::parse_rules(Cursor::new(
            r#"<query><rule name="//a:x" class="r"><list name="deps">
            <label value="//a:y"/><label value="//a:y"/></list></rule>
            <rule name="//a:y" class="r"/></query>"#,
        ));
        let snap = build(&rules, Status::Ok);
        assert_eq!(
            snap.edges,
            [Edge {
                from: "//a:x".into(),
                to: "//a:y".into(),
                attr: "deps".into()
            }]
        );
    }
}
