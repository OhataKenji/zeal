//! The target index and the relationships fetched for one selected target.
//!
//! The index has labels and rule classes. A detail query builds a temporary
//! full snapshot, then retains only the selected rule's edges and files.

use crate::label;
use crate::label::LabelParts;
use crate::xml::ParsedQuery;
use serde::Serialize;
use std::collections::HashSet;
use std::io::BufRead;
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

/// The small workspace-wide index sent to the tree and search UI.
#[derive(Clone, Debug, Serialize)]
pub struct TargetIndex {
    pub meta: Meta,
    pub targets: Vec<Target>,
}

/// Attributes and files for one selected target, derived from its own query.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetDetail {
    pub target: Target,
    pub edges: Vec<Edge>,
    pub files: Vec<FileEntry>,
    pub status: Status,
}

/// Parse Bazel's `<rule class> rule <label>` lines without XML attributes.
#[tracing::instrument(target = "zeal::operations", name = "query.label_kind_parse", skip_all)]
pub fn parse_label_kind(input: impl BufRead) -> Result<Vec<Target>, String> {
    let mut targets = Vec::new();
    for line in input.lines() {
        let line = line.map_err(|_| "could not read target list".to_string())?;
        let (rule_class, label) = line
            .rsplit_once(" rule ")
            .ok_or_else(|| "invalid target list output".to_string())?;
        if rule_class.is_empty() || !label.starts_with("//") {
            return Err("invalid target list output".to_string());
        }
        let label = label::canonical(label);
        let LabelParts { package, name } = label::split(&label);
        targets.push(Target {
            label,
            pkg: package,
            name,
            rule_class: rule_class.to_string(),
            location: None,
        });
    }
    Ok(targets)
}

/// Build the list generation held by the core until refresh.
#[tracing::instrument(target = "zeal::operations", name = "snapshot.index_build", skip_all)]
#[must_use]
pub fn build_index(mut targets: Vec<Target>, status: Status) -> TargetIndex {
    targets.sort_by(|a, b| a.label.cmp(&b.label));
    targets.dedup_by(|a, b| a.label == b.label);
    TargetIndex {
        meta: Meta {
            fetched_at: fetched_at(),
            status,
        },
        targets,
    }
}

/// Keep only facts visible in the selected target's details pane.
#[must_use]
pub fn select_detail(snapshot: Snapshot, label: &str) -> Option<TargetDetail> {
    let target = snapshot
        .targets
        .into_iter()
        .find(|target| target.label == label)?;
    Some(TargetDetail {
        target,
        edges: snapshot
            .edges
            .into_iter()
            .filter(|edge| edge.from == label)
            .collect(),
        files: snapshot
            .files
            .into_iter()
            .filter(|file| file.owner == label)
            .collect(),
        status: snapshot.meta.status,
    })
}

fn fetched_at() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Build a snapshot from parsed rules. Labels are canonicalized before being
/// classified: only explicit source-file metadata identifies input files.
/// Other references remain edges, including targets outside the selected scope.
#[tracing::instrument(target = "zeal::operations", name = "snapshot.build", skip_all)]
#[must_use]
pub fn build(query: &ParsedQuery, status: Status) -> Snapshot {
    let rules = &query.rules;
    let sources: HashSet<String> = query
        .source_files
        .iter()
        .map(|s| label::canonical(s))
        .collect();
    let mut targets = Vec::new();
    for r in rules {
        let lbl = label::canonical(&r.label);
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
            if label::is_external(&to) || !sources.contains(&to) {
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
            fetched_at: fetched_at(),
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

    #[test]
    fn label_kind_builds_only_tree_targets() {
        let targets = parse_label_kind(Cursor::new(
            "cc_test rule //app:test\njava_library rule //app/lib\n",
        ))
        .unwrap();
        let index = build_index(targets, Status::Ok);
        assert_eq!(index.targets.len(), 2);
        assert_eq!(index.targets[0].label, "//app/lib:lib");
        assert_eq!(index.targets[0].rule_class, "java_library");
        assert_eq!(index.targets[1].label, "//app:test");
        assert!(index.targets.iter().all(|target| target.location.is_none()));
        let json = serde_json::to_value(index).unwrap();
        assert!(json.get("edges").is_none());
        assert!(json.get("files").is_none());
        assert!(parse_label_kind(Cursor::new("not a rule\n")).is_err());
    }

    #[test]
    fn selected_detail_excludes_other_targets_relationships() {
        let query = xml::parse_query(Cursor::new(
            r#"<query>
              <rule class="r" name="//app:chosen">
                <list name="deps"><label value="//app:dep"/></list>
                <list name="srcs"><label value="//app:input.txt"/></list>
                <rule-output name="//app:out.txt"/>
              </rule>
              <rule class="r" name="//app:user"><list name="deps"><label value="//app:chosen"/></list></rule>
              <rule class="r" name="//app:other">
                <list name="deps"><label value="//app:else"/></list>
                <list name="srcs"><label value="//app:other.txt"/></list>
              </rule>
              <source-file name="//app:input.txt"/>
              <source-file name="//app:other.txt"/>
            </query>"#,
        ));
        let detail = select_detail(build(&query, Status::Ok), "//app:chosen").unwrap();
        assert_eq!(detail.edges.len(), 1);
        assert!(detail.edges.iter().all(|edge| edge.from == "//app:chosen"));
        assert_eq!(detail.files.len(), 2);
        assert!(detail.files.iter().all(|file| file.owner == "//app:chosen"));
        assert_eq!(detail.target.label, "//app:chosen");
    }

    fn fixture_rules() -> xml::ParsedQuery {
        let xml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/query.xml"
        ))
        .unwrap();
        xml::parse_query(Cursor::new(xml))
    }

    #[test]
    fn conditions_packages_keep_rule_and_file_inputs() {
        let rules = xml::parse_query(Cursor::new(
            r#"<query>
              <rule class="filegroup" name="//app:direct">
                <list name="srcs">
                  <label value="//conditions:inputs"/>
                  <label value="//conditions:input.txt"/>
                  <label value="//tools/conditions:input.txt"/>
                </list>
              </rule>
              <rule class="filegroup" name="//conditions:inputs"/>
              <source-file name="//conditions:input.txt"/>
              <source-file name="//tools/conditions:input.txt"/>
            </query>"#,
        ));
        let snapshot = build(&rules, Status::Ok);
        assert!(snapshot.edges.contains(&Edge {
            from: "//app:direct".into(),
            to: "//conditions:inputs".into(),
            attr: "srcs".into(),
        }));
        for path in ["conditions/input.txt", "tools/conditions/input.txt"] {
            assert!(snapshot.files.contains(&FileEntry {
                owner: "//app:direct".into(),
                path: path.into(),
                role: "srcs".into(),
            }));
        }
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
        // Non-deps/data attrs still become edges with their attribute name.
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
        let rules = xml::parse_query(Cursor::new(
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
    #[test]
    fn references_outside_scope_are_not_assumed_to_be_files() {
        let query = xml::parse_query(Cursor::new(
            r#"<query>
          <rule name="//app:main" class="r"><list name="srcs">
            <label value="//other:generated"/><label value="//app:main.rs"/>
          </list></rule>
          <source-file name="//app:main.rs"/>
        </query>"#,
        ));
        let snap = build(&query, Status::Ok);
        assert_eq!(snap.targets.len(), 1);
        assert_eq!(snap.edges[0].to, "//other:generated");
        assert_eq!(snap.files.len(), 1);
        assert_eq!(snap.files[0].path, "app/main.rs");
    }
}
