//! Configured-target Bazel reads for the inspector's Configured tab.
//!
//! `config("label", target)` names the target's own configuration under the
//! applied build flags; the same expression feeds the jsonproto summary,
//! the lite transitions report, and the files list, so retained copies of
//! the same label under other configurations cannot leak in. Each read is
//! one Bazel process — `--noblock_for_lock` applies per process and no
//! workspace lock is held across them.

use serde::{Deserialize, Serialize};
use std::io::BufRead;

/// Output paths shown on the target page before the "…" marker.
const MAX_OUTPUT_PATHS: usize = 10;

/// Dependency edges kept for the Configured tab; larger results set
/// `depsTruncated`.
const MAX_CONFIGURED_DEPS: usize = 200;

/// The selected target's own configuration, resolved by the `config()`
/// cquery expression under the applied build flags. `id` keys the
/// jsonproto `configurations` array — input only, never serialized.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSummary {
    #[serde(skip_serializing)]
    pub id: u32,
    pub mnemonic: String,
    pub checksum: String,
}

/// One direct dependency edge of the configured target. `transition` is a
/// short badge such as `exec`; `None` for edges that keep the parent's
/// configuration or have none (source files).
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredDep {
    pub label: String,
    pub transition: Option<String>,
}

/// `--output=files` result for the configured target.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetOutputs {
    pub paths: Vec<String>,
    pub truncated: bool,
}

/// The target's dependency edges, parsed from `cquery
/// --output=transitions --transitions=lite` over `config(...)`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetTransitions {
    pub deps: Vec<ConfiguredDep>,
    pub deps_truncated: bool,
}

/// `--output=files` prints one workspace-relative output path per line. Only
/// the first [`MAX_OUTPUT_PATHS`] are kept so filegroup-scale results stay
/// cheap; the rest are reported through `truncated`.
pub(crate) fn parse_output_paths(input: impl BufRead) -> Result<TargetOutputs, String> {
    let mut paths = Vec::new();
    let mut truncated = false;
    for line in input.lines() {
        let line = line.map_err(|_| "could not read cquery output".to_string())?;
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        if paths.len() < MAX_OUTPUT_PATHS {
            paths.push(path.to_string());
        } else {
            truncated = true;
        }
    }
    Ok(TargetOutputs { paths, truncated })
}

/// Parse the lite transitions report for the single `config()` result:
/// exactly one column-0 header followed by its indented
/// `attr#label#transition` rows. A missing header, a second column-0 line,
/// a row before the header, or a malformed row rejects the whole report;
/// stdout is always drained — dropping the pipe early can fail Bazel.
pub(crate) fn parse_transitions(mut input: impl BufRead) -> Result<TargetTransitions, String> {
    let parsed = parse_transition_section(&mut input);
    if parsed.is_err() {
        std::io::copy(&mut input, &mut std::io::sink())
            .map_err(|_| "could not read cquery output".to_string())?;
    }
    parsed
}

/// Fold the report's non-blank lines: the first must be the column-0
/// header, every later line an indented dependency row.
fn parse_transition_section(input: impl BufRead) -> Result<TargetTransitions, String> {
    const INVALID: &str = "unexpected cquery transitions output for a single target";
    let mut lines = input
        .lines()
        .map(|line| line.map_err(|_| "could not read cquery output".to_string()))
        .filter(|line| !matches!(line, Ok(s) if s.trim().is_empty()));
    let header = lines
        .next()
        .ok_or("cquery transitions output had no target header")??;
    if header.starts_with(char::is_whitespace) || !is_transitions_header(&header) {
        return Err(INVALID.into());
    }
    let mut deps = Vec::new();
    let mut deps_truncated = false;
    for line in lines {
        let line = line?;
        if !line.starts_with(char::is_whitespace) {
            return Err(INVALID.into());
        }
        let dep = parse_transition_row(&line).ok_or(INVALID)?;
        if deps.contains(&dep) {
            continue;
        }
        if deps.len() == MAX_CONFIGURED_DEPS {
            deps_truncated = true;
        } else {
            deps.push(dep);
        }
    }
    Ok(TargetTransitions {
        deps,
        deps_truncated,
    })
}

/// A column-0 transitions header ends with the configuration id in
/// parentheses, e.g. `NoTransition -> //app:t (9e6b91e)`. The label and
/// checksum are Bazel's to report; the parser never matches on them.
fn is_transitions_header(line: &str) -> bool {
    line.ends_with(')') && line.contains(" (")
}

/// One `attr#label#transition -> <ids>` row under a header. The attribute is
/// the first `#` field; labels may contain `#`, and Starlark transition
/// names carry a `.bzl` path that may too, so the split prefers the
/// `#(Starlark transition:` / `#Starlark transition:` boundary before
/// falling back to the last `#`. `(null transition)` and `NoTransition`
/// keep the parent's configuration; `exec` and `(exec + ...)` badge as
/// `exec`; every other name stays whole.
fn parse_transition_row(line: &str) -> Option<ConfiguredDep> {
    let (head, _) = line.rsplit_once("->")?;
    let (_, rest) = head.trim_end().split_once('#')?;
    let (label, name) = split_label_transition(rest)?;
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    let transition = match name.trim() {
        "(null transition)" | "NoTransition" => None,
        "exec" => Some("exec".to_string()),
        name if name.starts_with("(exec + ") => Some("exec".to_string()),
        name => Some(name.to_string()),
    };
    Some(ConfiguredDep {
        label: crate::label::normalize(label),
        transition,
    })
}

/// Split `label#transition` where the transition may itself contain `#`
/// inside a Starlark `.bzl` path. The Starlark boundary markers win over
/// the generic last-`#` split so `#` stays intact inside labels.
fn split_label_transition(rest: &str) -> Option<(&str, &str)> {
    for marker in ["#(Starlark transition:", "#Starlark transition:"] {
        if let Some(index) = rest.find(marker) {
            return Some((&rest[..index], &rest[index + 1..]));
        }
    }
    rest.rsplit_once('#')
}

/// One `cquery --output=jsonproto` result's configuration reference.
#[derive(Deserialize)]
struct ResultConfig {
    #[serde(rename = "configurationId", default)]
    id: u32,
}

/// `cquery --output=jsonproto` top level: the results plus the named
/// configurations they reference. Only the fields [`ConfigSummary`] declares
/// are read — Bazel's remaining option payload is ignored.
#[derive(Deserialize)]
struct ConfigQuery {
    #[serde(default)]
    results: Vec<ResultConfig>,
    #[serde(default)]
    configurations: Vec<ConfigSummary>,
}

/// `config("label", target)` answers with exactly one result; its
/// `configurationId` names the entry in `configurations` — absent or zero
/// for targets in the null configuration (source files).
pub(crate) fn parse_configuration(input: impl BufRead) -> Result<Option<ConfigSummary>, String> {
    let query: ConfigQuery = serde_json::from_reader(input)
        .map_err(|e| format!("could not parse cquery configuration output: {e}"))?;
    if query.results.len() != 1 {
        return Err(format!(
            "cquery config() returned {} results",
            query.results.len()
        ));
    }
    let id = query.results[0].id;
    if id == 0 {
        return Ok(None);
    }
    query
        .configurations
        .into_iter()
        .find(|config| config.id == id)
        .map(Some)
        .ok_or_else(|| format!("cquery config() referenced unknown configuration {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `config(label, target)` lite-transitions section in the shape
    /// Bazel reports: a single header plus the target's own dependency
    /// rows.
    const ROOT_TRANSITIONS: &str = "\
NoTransition -> //app:chosen (9e6b91e)
  srcs#//app:input.txt#(null transition) ->
  tools#//app:tool#(exec + (TestTrimmingTransition + ConfigFeatureFlagTaggedTrimmingTransition)) -> f50cc19
  $genrule_setup#@bazel_tools//tools/genrule:genrule-setup.sh#(null transition) ->
";

    fn dep_pairs(parsed: &TargetTransitions) -> Vec<(&str, Option<&str>)> {
        parsed
            .deps
            .iter()
            .map(|dep| (dep.label.as_str(), dep.transition.as_deref()))
            .collect()
    }

    #[test]
    fn output_paths_skip_blank_lines_and_cap_the_list() {
        let mut input = String::new();
        for i in 0..MAX_OUTPUT_PATHS + 3 {
            input.push_str(&format!("bazel-out/k8/bin/app/out{i}\n"));
        }
        input.push_str("\n  bazel-out/k8/bin/app/last \n");
        let outputs = parse_output_paths(std::io::Cursor::new(input)).unwrap();
        assert_eq!(outputs.paths.len(), MAX_OUTPUT_PATHS);
        assert_eq!(outputs.paths[0], "bazel-out/k8/bin/app/out0");
        assert!(outputs.truncated);

        let outputs = parse_output_paths(std::io::Cursor::new(
            "bazel-out/k8/bin/app/server\n\nbazel-out/k8/bin/app/lib.a\n",
        ))
        .unwrap();
        assert_eq!(outputs.paths.len(), 2);
        assert!(!outputs.truncated);
    }

    #[test]
    fn transitions_parse_the_single_root_section() {
        let parsed = parse_transitions(ROOT_TRANSITIONS.as_bytes()).unwrap();
        assert!(!parsed.deps_truncated);
        assert_eq!(
            dep_pairs(&parsed),
            [
                ("//app:input.txt", None),
                ("//app:tool", Some("exec")),
                ("@bazel_tools//tools/genrule:genrule-setup.sh", None),
            ]
        );
    }

    #[test]
    fn transitions_keep_label_hashes_external_and_starlark_boundaries() {
        // cquery may print the main repository as @@//; it means // here,
        // while @@repo stays canonical and distinct from @repo. Labels may
        // contain '#', and a Starlark transition's .bzl path may too — the
        // #(Starlark transition: boundary, not the last '#', splits.
        let input = "\
NoTransition -> //app:a (aaa1111)
  deps#@@//app:x#NoTransition ->
  deps#@@rules_cc//cc:defs.bzl%defs#NoTransition ->
  deps#@repo//pkg:y#NoTransition ->
  deps#//pkg:x#y#(null transition) ->
  deps#//pkg:x#y#(Starlark transition:/ws/rules#v2.bzl:4:40 + (TestTrimmingTransition)) ->
";
        let parsed = parse_transitions(input.as_bytes()).unwrap();
        assert_eq!(
            dep_pairs(&parsed),
            [
                ("//app:x", None),
                ("@@rules_cc//cc:defs.bzl%defs", None),
                ("@repo//pkg:y", None),
                ("//pkg:x#y", None),
                (
                    "//pkg:x#y",
                    Some("(Starlark transition:/ws/rules#v2.bzl:4:40 + (TestTrimmingTransition))"),
                ),
            ]
        );
    }

    #[test]
    fn transitions_dedupe_rows_and_skip_blank_lines() {
        // Blank lines neither start a row nor end the section; duplicate
        // rows collapse into one edge.
        let input = "\
NoTransition -> //app:a (aaa1111)

  deps#//app:shared#(exec + (Trim)) -> bbb2222
  deps#//app:shared#(exec + (Trim)) -> bbb2222

  srcs#//app:one#NoTransition ->
  data#//app:two#(null transition) ->
";
        let parsed = parse_transitions(input.as_bytes()).unwrap();
        assert_eq!(
            dep_pairs(&parsed),
            [
                ("//app:shared", Some("exec")),
                ("//app:one", None),
                ("//app:two", None),
            ]
        );
    }

    #[test]
    fn transitions_reject_malformed_or_ambiguous_output() {
        // A second column-0 section, a row before the header, a malformed
        // row, and a non-header first line all reject the report; the
        // diagnostic is fixed and never echoes Bazel's line.
        for input in [
            "NoTransition -> //app:a (aaa1111)\n  deps#//app:x#NoTransition ->\nNoTransition -> //app:b (bbb2222)\n",
            "  deps#//app:x#NoTransition ->\nNoTransition -> //app:a (aaa1111)\n",
            "NoTransition -> //app:a (aaa1111)\n  this is not a row\n",
            "some unexpected output\n  deps#//app:x#NoTransition ->\n",
        ] {
            let error = parse_transitions(input.as_bytes()).unwrap_err();
            assert_eq!(
                error, "unexpected cquery transitions output for a single target",
                "{input:?}"
            );
        }
        // Empty output has no header at all.
        assert_eq!(
            parse_transitions("".as_bytes()).unwrap_err(),
            "cquery transitions output had no target header"
        );
        // A header with no rows is a valid empty dependency list.
        let parsed = parse_transitions("NoTransition -> //app:a (aaa1111)\n".as_bytes()).unwrap();
        assert!(parsed.deps.is_empty());
        assert!(!parsed.deps_truncated);
    }

    #[test]
    fn transition_rows_stop_at_the_dependency_cap() {
        let mut input = String::from("NoTransition -> //app:a (aaa1111)\n");
        for i in 0..MAX_CONFIGURED_DEPS + 5 {
            input.push_str(&format!("  deps#//app:d{i}#NoTransition ->\n"));
        }
        let parsed = parse_transitions(input.as_bytes()).unwrap();
        assert_eq!(parsed.deps.len(), MAX_CONFIGURED_DEPS);
        assert!(parsed.deps_truncated);
    }

    #[test]
    fn transition_rows_mark_only_exec_and_keep_other_names_whole() {
        let badge = |name: &str| {
            parse_transition_row(&format!("  deps#//app:x#{name} ->"))
                .unwrap()
                .transition
        };
        assert_eq!(badge("(null transition)"), None);
        assert_eq!(badge("NoTransition"), None);
        assert_eq!(badge("exec"), Some("exec".to_string()));
        assert_eq!(
            badge("(exec + (TestTrimmingTransition + ConfigFeatureFlagTaggedTrimmingTransition))"),
            Some("exec".to_string())
        );
        // Non-exec transition names stay whole so the badge stays honest.
        let starlark =
            "(Starlark transition:/ws/app/transition.bzl:4:40 + (TestTrimmingTransition))";
        assert_eq!(badge(starlark), Some(starlark.to_string()));
    }

    #[test]
    fn configuration_lookup_matches_by_id_not_position() {
        // The matching configuration is the second entry; options payloads
        // are present but never materialized into the summary.
        let json = r#"{
            "results": [
                {"target": {"type": "RULE"}, "configurationId": 2,
                 "configuration": {"checksum": "bbbb"}}
            ],
            "configurations": [
                {"id": 1, "mnemonic": "k8-fastbuild",
                 "checksum": "aaaaaaaaaaaaaaaa",
                 "options": {"compilation_mode": "fastbuild", "huge": "payload"},
                 "platformName": "@//:host"},
                {"id": 2, "mnemonic": "k8-opt",
                 "checksum": "bbbbbbbbbbbbbbbb",
                 "options": {"compilation_mode": "opt"}}
            ]
        }"#;
        let config = parse_configuration(std::io::Cursor::new(json))
            .unwrap()
            .unwrap();
        assert_eq!(config.mnemonic, "k8-opt");
        assert_eq!(config.checksum, "bbbbbbbbbbbbbbbb");
    }

    #[test]
    fn configuration_lookup_handles_null_and_rejects_ambiguity() {
        // No configurationId (source file) means the null configuration.
        let null = parse_configuration(std::io::Cursor::new(
            r#"{"results": [{"target": {"type": "SOURCE_FILE"}}], "configurations": []}"#,
        ))
        .unwrap();
        assert!(null.is_none());
        // Not exactly one result, or an id missing from configurations, fails.
        assert!(
            parse_configuration(std::io::Cursor::new(
                r#"{"results": [], "configurations": []}"#
            ))
            .is_err()
        );
        assert!(
            parse_configuration(std::io::Cursor::new(
                r#"{"results": [{"configurationId": 1}, {"configurationId": 2}], "configurations": []}"#
            ))
            .is_err()
        );
        assert!(
            parse_configuration(std::io::Cursor::new(
                r#"{"results": [{"configurationId": 9}], "configurations": [{"id": 1, "mnemonic": "m", "checksum": "c"}]}"#
            ))
            .is_err()
        );
    }
}
