//! The target index and the relationships fetched for one selected target.
//!
//! The index has labels and rule classes. A detail query builds a temporary
//! full snapshot, then retains only the selected rule's edges and files.

use crate::label;
use crate::label::LabelParts;
use crate::query_proto::{ParsedQuery, RawRule};
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

/// Parse Bazel's `<rule class> rule <label>` lines.
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
        let label = label::normalize(label);
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

/// Build a snapshot from parsed rules. Labels are normalized before being
/// classified: only explicit source-file metadata identifies input files.
/// Other references remain edges, including targets outside the selected scope.
#[tracing::instrument(target = "zeal::operations", name = "snapshot.build", skip_all)]
#[must_use]
pub fn build(query: &ParsedQuery, status: Status) -> Snapshot {
    let sources: HashSet<String> = query
        .source_files
        .iter()
        .map(|s| label::normalize(s))
        .collect();
    let (edges, files) = relationships(&query.rules, &sources);
    Snapshot {
        meta: Meta {
            fetched_at: fetched_at(),
            status,
        },
        targets: rule_targets(&query.rules),
        edges,
        files,
    }
}

/// Sorted target rows, one per parsed rule.
fn rule_targets(rules: &[RawRule]) -> Vec<Target> {
    let mut targets = Vec::new();
    for r in rules {
        let lbl = label::normalize(&r.label);
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
    targets
}

/// Edges and file entries from every rule's dep-ish attrs and declared
/// outputs. `sources` is the normalized label set of the source files.
fn relationships(rules: &[RawRule], sources: &HashSet<String>) -> (Vec<Edge>, Vec<FileEntry>) {
    let mut edges = HashSet::new();
    let mut files = HashSet::new();
    for r in rules {
        rule_relationships(r, sources, &mut edges, &mut files);
    }

    let mut edges: Vec<Edge> = edges.into_iter().collect();
    edges.sort_by(|a, b| (&a.from, &a.attr, &a.to).cmp(&(&b.from, &b.attr, &b.to)));
    let mut files: Vec<FileEntry> = files.into_iter().collect();
    files.sort_by(|a, b| (&a.path, &a.owner, &a.role).cmp(&(&b.path, &b.owner, &b.role)));
    (edges, files)
}

/// One rule's references: dep-ish attrs land as edges or file entries, and
/// declared outputs are always file entries.
fn rule_relationships(
    r: &RawRule,
    sources: &HashSet<String>,
    edges: &mut HashSet<Edge>,
    files: &mut HashSet<FileEntry>,
) {
    let owner = label::normalize(&r.label);
    for (attr, raw) in &r.label_attrs {
        let Some(to) = dep_label(attr, raw) else {
            continue;
        };
        if label::is_external(&to) || !sources.contains(&to) {
            edges.insert(Edge {
                from: owner.clone(),
                to,
                attr: attr.clone(),
            });
            continue;
        }
        files.insert(FileEntry {
            path: label::label_to_path(&to),
            owner: owner.clone(),
            role: attr.clone(),
        });
    }
    // Outputs come only from `rule_output`: output-typed attributes like
    // `outs` are not label kinds and never become label_attrs.
    for out in &r.outputs {
        files.insert(FileEntry {
            path: label::label_to_path(&label::normalize(out)),
            owner: owner.clone(),
            role: "out".to_string(),
        });
    }
}

/// The normalized label a dep-ish attribute points at, or `None` for attrs and
/// select() branch keys that never become relationships.
fn dep_label(attr: &str, raw: &str) -> Option<String> {
    if !label::is_dep_attr(attr) {
        return None;
    }
    let to = label::normalize(raw);
    if label::is_select_key(&to) {
        return None;
    }
    Some(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn rule(label: &str) -> RawRule {
        RawRule {
            label: label.into(),
            rule_class: "r".into(),
            ..Default::default()
        }
    }

    fn attrs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect()
    }

    #[test]
    fn label_kind_parses_rules_into_sorted_targets() {
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
    }

    #[test]
    fn target_index_json_omits_snapshot_fields() {
        let targets = parse_label_kind(Cursor::new("cc_test rule //app:test\n")).unwrap();
        let json = serde_json::to_value(build_index(targets, Status::Ok)).unwrap();
        assert!(json.get("edges").is_none());
        assert!(json.get("files").is_none());
    }

    #[test]
    fn label_kind_rejects_lines_without_rule_marker() {
        assert!(parse_label_kind(Cursor::new("not a rule\n")).is_err());
    }

    #[test]
    fn selected_detail_excludes_other_targets_relationships() {
        let query = ParsedQuery {
            rules: vec![
                RawRule {
                    label_attrs: attrs(&[("deps", "//app:dep"), ("srcs", "//app:input.txt")]),
                    outputs: vec!["//app:out.txt".into()],
                    ..rule("//app:chosen")
                },
                RawRule {
                    label_attrs: attrs(&[("deps", "//app:chosen")]),
                    ..rule("//app:user")
                },
                RawRule {
                    label_attrs: attrs(&[("deps", "//app:else"), ("srcs", "//app:other.txt")]),
                    ..rule("//app:other")
                },
            ],
            source_files: vec!["//app:input.txt".into(), "//app:other.txt".into()],
        };
        let detail = select_detail(build(&query, Status::Ok), "//app:chosen").unwrap();
        assert_eq!(detail.edges.len(), 1);
        assert!(detail.edges.iter().all(|edge| edge.from == "//app:chosen"));
        assert_eq!(detail.files.len(), 2);
        assert!(detail.files.iter().all(|file| file.owner == "//app:chosen"));
        assert_eq!(detail.target.label, "//app:chosen");
    }

    #[test]
    fn conditions_packages_keep_rule_and_file_inputs() {
        let query = ParsedQuery {
            rules: vec![
                RawRule {
                    label_attrs: attrs(&[
                        ("srcs", "//conditions:inputs"),
                        ("srcs", "//conditions:input.txt"),
                        ("srcs", "//tools/conditions:input.txt"),
                    ]),
                    ..rule("//app:direct")
                },
                rule("//conditions:inputs"),
            ],
            source_files: vec![
                "//conditions:input.txt".into(),
                "//tools/conditions:input.txt".into(),
            ],
        };
        let snapshot = build(&query, Status::Ok);
        assert!(snapshot.edges.contains(&Edge {
            from: "//app:direct".into(),
            to: "//conditions:inputs".into(),
            attr: "srcs".into(),
        }));
        assert_eq!(
            snapshot.files,
            [
                FileEntry {
                    owner: "//app:direct".into(),
                    path: "conditions/input.txt".into(),
                    role: "srcs".into(),
                },
                FileEntry {
                    owner: "//app:direct".into(),
                    path: "tools/conditions/input.txt".into(),
                    role: "srcs".into(),
                },
            ]
        );
    }

    #[test]
    fn snapshot_targets_normalize_bare_labels() {
        let query = ParsedQuery {
            rules: vec![rule("//app")],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert_eq!(snapshot.targets.len(), 1);
        // `//app` normalizes to `//app:app`.
        assert_eq!(snapshot.targets[0].label, "//app:app");
    }

    #[test]
    fn deps_attr_produces_an_edge() {
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[("deps", "//app:lib")]),
                ..rule("//app:main")
            }],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert_eq!(
            snapshot.edges,
            [Edge {
                from: "//app:main".into(),
                to: "//app:lib".into(),
                attr: "deps".into(),
            }]
        );
    }

    #[test]
    fn external_deps_keep_their_canonical_label() {
        // External canonical names are not apparent names without a repo mapping.
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[("deps", "@@maven+//:com_google_guava_guava")]),
                ..rule("//app/lib:parser")
            }],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert_eq!(
            snapshot.edges,
            [Edge {
                from: "//app/lib:parser".into(),
                to: "@@maven+//:com_google_guava_guava".into(),
                attr: "deps".into(),
            }]
        );
    }

    #[test]
    fn data_pointing_at_a_rule_is_an_edge() {
        // `data` pointing at a rule is an edge, not a file.
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[("data", "//app:grammars")]),
                ..rule("//app/lib:parser")
            }],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert_eq!(
            snapshot.edges,
            [Edge {
                from: "//app/lib:parser".into(),
                to: "//app:grammars".into(),
                attr: "data".into(),
            }]
        );
    }

    #[test]
    fn tools_attr_produces_an_edge() {
        // Non-deps/data attrs still become edges with their attribute name.
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[("tools", "//tools:launcher")]),
                ..rule("//app:app")
            }],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert_eq!(
            snapshot.edges,
            [Edge {
                from: "//app:app".into(),
                to: "//tools:launcher".into(),
                attr: "tools".into(),
            }]
        );
    }

    #[test]
    fn visibility_labels_are_not_dependencies() {
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[("visibility", "//visibility:public")]),
                ..rule("//app:main")
            }],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert!(snapshot.edges.is_empty());
        assert!(snapshot.files.is_empty());
    }

    #[test]
    fn implicit_attributes_are_not_dependencies() {
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[("$cc_toolchain", "//tools:cc")]),
                ..rule("//app:main")
            }],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert!(snapshot.edges.is_empty());
        assert!(snapshot.files.is_empty());
    }

    #[test]
    fn select_default_keys_are_not_dependencies() {
        // Excluded: select() branch keys.
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[("deps_opt", "//conditions:default")]),
                ..rule("//app/lib:parser")
            }],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert!(snapshot.edges.is_empty());
        assert!(snapshot.files.is_empty());
    }

    #[test]
    fn shared_source_file_records_every_owner() {
        let query = ParsedQuery {
            rules: vec![
                RawRule {
                    label_attrs: attrs(&[("srcs", "//app:file.txt")]),
                    ..rule("//app:a")
                },
                RawRule {
                    label_attrs: attrs(&[("srcs", "//app:file.txt")]),
                    ..rule("//app:b")
                },
            ],
            source_files: vec!["//app:file.txt".into()],
        };
        let snapshot = build(&query, Status::Ok);
        // File referenced by two rules' srcs has both owners.
        assert_eq!(
            snapshot.files,
            [
                FileEntry {
                    path: "app/file.txt".into(),
                    owner: "//app:a".into(),
                    role: "srcs".into(),
                },
                FileEntry {
                    path: "app/file.txt".into(),
                    owner: "//app:b".into(),
                    role: "srcs".into(),
                },
            ]
        );
    }

    #[test]
    fn data_input_is_a_file_with_data_role() {
        // `data` label to a file lands in files with role `data`.
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[("data", "//app:config.txt")]),
                ..rule("//app:main")
            }],
            source_files: vec!["//app:config.txt".into()],
        };
        let snapshot = build(&query, Status::Ok);
        assert_eq!(
            snapshot.files,
            [FileEntry {
                path: "app/config.txt".into(),
                owner: "//app:main".into(),
                role: "data".into(),
            }]
        );
    }

    #[test]
    fn declared_outputs_are_files_with_out_role() {
        let query = ParsedQuery {
            rules: vec![RawRule {
                outputs: vec!["//app:out.txt".into()],
                ..rule("//app:main")
            }],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert_eq!(
            snapshot.files,
            [FileEntry {
                path: "app/out.txt".into(),
                owner: "//app:main".into(),
                role: "out".into(),
            }]
        );
    }

    #[test]
    fn file_entries_are_unique_per_path_owner_role() {
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[
                    ("srcs", "//app:file.txt"),
                    ("srcs", "//app:file.txt"),
                    ("data", "//app:file.txt"),
                ]),
                ..rule("//app:main")
            }],
            source_files: vec!["//app:file.txt".into()],
        };
        let snapshot = build(&query, Status::Ok);
        assert_eq!(
            snapshot.files,
            [
                FileEntry {
                    path: "app/file.txt".into(),
                    owner: "//app:main".into(),
                    role: "data".into(),
                },
                FileEntry {
                    path: "app/file.txt".into(),
                    owner: "//app:main".into(),
                    role: "srcs".into(),
                },
            ]
        );
    }

    #[test]
    fn edges_dedup() {
        let query = ParsedQuery {
            rules: vec![
                RawRule {
                    label_attrs: attrs(&[("deps", "//a:y"), ("deps", "//a:y")]),
                    ..rule("//a:x")
                },
                rule("//a:y"),
            ],
            ..Default::default()
        };
        let snapshot = build(&query, Status::Ok);
        assert_eq!(
            snapshot.edges,
            [Edge {
                from: "//a:x".into(),
                to: "//a:y".into(),
                attr: "deps".into()
            }]
        );
    }

    #[test]
    fn references_outside_scope_are_not_assumed_to_be_files() {
        let query = ParsedQuery {
            rules: vec![RawRule {
                label_attrs: attrs(&[("srcs", "//other:generated"), ("srcs", "//app:main.rs")]),
                ..rule("//app:main")
            }],
            source_files: vec!["//app:main.rs".into()],
        };
        let snap = build(&query, Status::Ok);
        assert_eq!(snap.targets.len(), 1);
        assert_eq!(snap.edges[0].to, "//other:generated");
        assert_eq!(snap.files.len(), 1);
        assert_eq!(snap.files[0].path, "app/main.rs");
    }
}
