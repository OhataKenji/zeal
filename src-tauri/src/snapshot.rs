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
    use std::collections::HashMap;
    use std::io::Cursor;

    fn rule(
        label: &str,
        class: &str,
        location: Option<(&str, u32)>,
        attrs: &[(&str, &str)],
        outputs: &[&str],
    ) -> RawRule {
        RawRule {
            label: label.to_string(),
            rule_class: class.to_string(),
            location: location.map(|(file, line)| (file.to_string(), line)),
            label_attrs: attrs
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            outputs: outputs.iter().map(|output| output.to_string()).collect(),
        }
    }

    /// The internal shape the retired `query.xml` fixture parsed to: seven
    /// rules and five source files.
    fn fixture_rules() -> ParsedQuery {
        ParsedQuery {
            rules: vec![
                rule(
                    "//app/lib:parser",
                    "java_library",
                    Some(("/repo/app/lib/BUILD", 3)),
                    &[
                        ("srcs", "//app/lib:Parser.java"),
                        ("srcs", "//app/lib:Token.java"),
                        ("deps", "//app/lib:parser_proto"),
                        ("deps", "@@maven+//:com_google_guava_guava"),
                        ("data", "//app:grammars"),
                        ("data", "//app/lib:grammars.txt"),
                        ("visibility", "//visibility:public"),
                        ("$cc_toolchain", "//tools:cc"),
                        ("deps_opt", "//conditions:default"),
                        ("deps_opt", "//app/lib:parser_proto"),
                    ],
                    &["//app/lib:libparser.jar"],
                ),
                rule(
                    "//app/lib:parser_proto",
                    "proto_library",
                    Some(("/repo/app/lib/BUILD", 10)),
                    &[("srcs", "//app/lib:parser.proto")],
                    &["//app/lib:libparser_proto_proto-descriptor-set.proto.bin"],
                ),
                rule(
                    "//app/lib:parser_test",
                    "java_test",
                    Some(("/repo/app/lib/BUILD", 15)),
                    &[
                        ("srcs", "//app/lib:Parser.java"),
                        ("deps", "//app/lib:parser"),
                    ],
                    &[],
                ),
                rule(
                    "//app",
                    "java_binary",
                    Some(("/repo/app/BUILD", 2)),
                    &[
                        ("deps", "//app/lib:parser"),
                        ("data", "//app:server.cfg"),
                        ("tools", "//tools:launcher"),
                    ],
                    &["//app:server.jar"],
                ),
                rule(
                    "//app:grammars",
                    "filegroup",
                    Some(("/repo/app/BUILD", 9)),
                    &[("srcs", "//app:g1.txt"), ("srcs", "//app:g2.txt")],
                    &[],
                ),
                rule(
                    "//tools:launcher",
                    "genrule",
                    Some(("/repo/tools/build_defs.bzl", 22)),
                    &[],
                    &["//tools:launcher.sh"],
                ),
                rule(
                    "//tools:cc",
                    "cc_library",
                    Some(("/repo/tools/BUILD", 4)),
                    &[],
                    &[],
                ),
            ],
            source_files: [
                "//app/lib:Parser.java",
                "//app/lib:Token.java",
                "//app/lib:grammars.txt",
                "//app/lib:parser.proto",
                "//app:server.cfg",
            ]
            .iter()
            .map(|name| name.to_string())
            .collect(),
        }
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
                rule(
                    "//app:chosen",
                    "r",
                    None,
                    &[("deps", "//app:dep"), ("srcs", "//app:input.txt")],
                    &["//app:out.txt"],
                ),
                rule("//app:user", "r", None, &[("deps", "//app:chosen")], &[]),
                rule(
                    "//app:other",
                    "r",
                    None,
                    &[("deps", "//app:else"), ("srcs", "//app:other.txt")],
                    &[],
                ),
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
        let rules = ParsedQuery {
            rules: vec![
                rule(
                    "//app:direct",
                    "filegroup",
                    None,
                    &[
                        ("srcs", "//conditions:inputs"),
                        ("srcs", "//conditions:input.txt"),
                        ("srcs", "//tools/conditions:input.txt"),
                    ],
                    &[],
                ),
                rule("//conditions:inputs", "filegroup", None, &[], &[]),
            ],
            source_files: vec![
                "//conditions:input.txt".into(),
                "//tools/conditions:input.txt".into(),
            ],
        };
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
    fn snapshot_targets_normalize_bare_labels() {
        let snap = build(&fixture_rules(), Status::Ok);
        let labels: HashSet<&str> = snap.targets.iter().map(|t| t.label.as_str()).collect();
        // `//app` normalizes to `//app:app`.
        assert!(labels.contains("//app:app"));
        assert!(!labels.contains("//app"));
    }

    fn edge_set(snap: &Snapshot) -> HashSet<(&str, &str, &str)> {
        snap.edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str(), e.attr.as_str()))
            .collect()
    }

    #[test]
    fn dep_attrs_produce_edges_under_their_attr_name() {
        let snap = build(&fixture_rules(), Status::Ok);
        let edges = edge_set(&snap);

        assert!(edges.contains(&("//app/lib:parser", "//app/lib:parser_proto", "deps")));
        // External canonical names are not apparent names without a repo mapping.
        assert!(edges.contains(&(
            "//app/lib:parser",
            "@@maven+//:com_google_guava_guava",
            "deps"
        )));
        // `data` pointing at a rule is an edge, not a file.
        assert!(edges.contains(&("//app/lib:parser", "//app:grammars", "data")));
        // Non-deps/data attrs still become edges with their attribute name.
        assert!(edges.contains(&("//app:app", "//tools:launcher", "tools")));
        assert!(edges.contains(&("//app/lib:parser_test", "//app/lib:parser", "deps")));
    }

    #[test]
    fn non_dep_attrs_produce_no_edges() {
        let snap = build(&fixture_rules(), Status::Ok);
        let edges = edge_set(&snap);

        // Excluded: visibility, implicit `$` attrs, select() branch keys.
        assert!(!edges.iter().any(|e| e.1 == "//visibility:public"));
        assert!(!edges.iter().any(|e| e.2 == "$cc_toolchain"));
        assert!(!edges.iter().any(|e| e.1.contains("conditions")));
    }

    /// File entries grouped by path as (owner, role) pairs.
    fn owners_by_path(snap: &Snapshot) -> HashMap<&str, Vec<(&str, &str)>> {
        let mut owners: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        for f in &snap.files {
            owners
                .entry(f.path.as_str())
                .or_default()
                .push((f.owner.as_str(), f.role.as_str()));
        }
        owners
    }

    #[test]
    fn file_inputs_carry_owner_and_attr_role() {
        let snap = build(&fixture_rules(), Status::Ok);
        let owners = owners_by_path(&snap);

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

        // rule-input alone never creates a file entry.
        assert!(!owners.contains_key("tools/BUILD"));
    }

    #[test]
    fn declared_outputs_are_files_with_out_role() {
        let snap = build(&fixture_rules(), Status::Ok);
        let owners = owners_by_path(&snap);

        // Outputs carry role `out`; a genrule's `outs` is an output-typed
        // attribute, so its file entry comes only from `rule_output`.
        assert_eq!(
            owners["app/lib/libparser.jar"],
            [("//app/lib:parser", "out")]
        );
        assert_eq!(owners["tools/launcher.sh"], [("//tools:launcher", "out")]);
    }

    #[test]
    fn file_entries_are_unique_per_path_owner_role() {
        let snap = build(&fixture_rules(), Status::Ok);
        let uniq: HashSet<(&str, &str, &str)> = snap
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.owner.as_str(), f.role.as_str()))
            .collect();
        assert_eq!(uniq.len(), snap.files.len());
    }

    #[test]
    fn edges_dedup() {
        let rules = ParsedQuery {
            rules: vec![
                rule(
                    "//a:x",
                    "r",
                    None,
                    &[("deps", "//a:y"), ("deps", "//a:y")],
                    &[],
                ),
                rule("//a:y", "r", None, &[], &[]),
            ],
            source_files: vec![],
        };
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
        let query = ParsedQuery {
            rules: vec![rule(
                "//app:main",
                "r",
                None,
                &[("srcs", "//other:generated"), ("srcs", "//app:main.rs")],
                &[],
            )],
            source_files: vec!["//app:main.rs".into()],
        };
        let snap = build(&query, Status::Ok);
        assert_eq!(snap.targets.len(), 1);
        assert_eq!(snap.edges[0].to, "//other:generated");
        assert_eq!(snap.files.len(), 1);
        assert_eq!(snap.files[0].path, "app/main.rs");
    }
}
