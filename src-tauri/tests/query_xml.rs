//! Golden test for the query XML parser and snapshot builder.
//! The fixture covers: select() branch keys, a macro-generated rule (location
//! in a .bzl file), an external-repo label, file labels in `data`, rules with
//! and without `<rule-output>`, and a file with multiple owners.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use zeal_lib::snapshot::{self, Edge, Status};
use zeal_lib::xml;

fn fixture_rules() -> Vec<xml::RawRule> {
    let xml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/query.xml"
    ))
    .unwrap();
    xml::parse_rules(Cursor::new(xml))
}

#[test]
fn parses_all_rules() {
    let rules = fixture_rules();
    let labels: HashSet<&str> = rules.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "//app/lib:parser",
            "//app/lib:parser_proto",
            "//app/lib:parser_test",
            "//app",
            "//app:grammars",
            "//tools:launcher",
            "//tools:cc",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn parses_class_location_and_outputs() {
    let rules = fixture_rules();
    let by_label: HashMap<&str, &xml::RawRule> =
        rules.iter().map(|r| (r.label.as_str(), r)).collect();

    let parser = by_label["//app/lib:parser"];
    assert_eq!(parser.rule_class, "java_library");
    assert_eq!(
        parser.location,
        Some(("/repo/app/lib/BUILD".to_string(), 3))
    );
    assert_eq!(parser.outputs, ["//app/lib:libparser.jar"]);

    // Macro-generated rule: location lands inside a .bzl file.
    let launcher = by_label["//tools:launcher"];
    assert_eq!(
        launcher.location,
        Some(("/repo/tools/build_defs.bzl".to_string(), 22))
    );

    // A rule that declares no outputs has none.
    assert!(by_label["//app/lib:parser_test"].outputs.is_empty());
}

#[test]
fn snapshot_targets_canonicalize_bare_labels() {
    let snap = snapshot::build(&fixture_rules(), Status::Ok);
    let labels: HashSet<&str> = snap.targets.iter().map(|t| t.label.as_str()).collect();
    // `//app` canonicalizes to `//app:app`.
    assert!(labels.contains("//app:app"));
    assert!(!labels.contains("//app"));
}

#[test]
fn snapshot_edges_only_from_dep_attrs() {
    let snap = snapshot::build(&fixture_rules(), Status::Ok);
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
    let snap = snapshot::build(&fixture_rules(), Status::Ok);
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
fn truncated_xml_yields_completed_rules() {
    let xml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/query.xml"
    ))
    .unwrap();
    // Cut mid-rule: everything after the first </rule> is lost.
    let cut = xml.find("</rule>").unwrap() + "</rule>".len();
    let rules = xml::parse_rules(Cursor::new(&xml[..cut]));
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].label, "//app/lib:parser");

    // Cut mid-body: the rule survives with the attributes that arrived.
    let mid = xml.find("<list name=\"deps\">").unwrap();
    let rules = xml::parse_rules(Cursor::new(&xml[..mid]));
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].label, "//app/lib:parser");
    assert!(!rules[0].label_attrs.iter().any(|(a, _)| a == "deps"));
}

#[test]
fn edges_dedup() {
    let rules = xml::parse_rules(Cursor::new(
        r#"<query><rule name="//a:x" class="r"><list name="deps">
        <label value="//a:y"/><label value="//a:y"/></list></rule>
        <rule name="//a:y" class="r"/></query>"#,
    ));
    let snap = snapshot::build(&rules, Status::Ok);
    assert_eq!(
        snap.edges,
        [Edge {
            from: "//a:x".into(),
            to: "//a:y".into(),
            attr: "deps".into()
        }]
    );
}
