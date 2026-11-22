//! Configured-target Bazel reads for the inspector's Configured tab.
//!
//! `config("label", target)` names the target's own configuration under the
//! applied build flags; `deps(config(...), 1)` transitions and
//! `config(...)` files are both scoped to that exact configuration, so
//! retained copies of the same label under other configurations cannot
//! leak in. Each read is one Bazel process — `--noblock_for_lock` applies
//! per process and no workspace lock is held across them.

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
/// --output=transitions --transitions=lite` over `deps(config(...))`.
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

/// Parse the transitions report for `label` under `checksum` — the full
/// config hash `config()` reported, or `None` for the null configuration.
/// `deps(config(...))` yields exactly one matching header; same-label
/// headers under other retained configurations are ignored. stdout is
/// always drained — dropping the pipe early can fail Bazel.
pub(crate) fn parse_transitions(
    input: impl BufRead,
    label: &str,
    checksum: Option<&str>,
) -> Result<TargetTransitions, String> {
    let mut found = false;
    let mut collecting = false;
    let mut malformed = false;
    let mut deps = Vec::new();
    let mut deps_truncated = false;
    for line in input.lines() {
        let line = line.map_err(|_| "could not read cquery output".to_string())?;
        // Blank lines are skipped before header detection: they neither
        // start a row nor end the selected header's section.
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            // Only the first matching header collects; any later header —
            // even a repeat — ends collection for good.
            collecting = !found && is_target_header(&line, label, checksum);
            found |= collecting;
            continue;
        }
        // A malformed row is recorded but stdout keeps draining: dropping
        // the pipe early can fail Bazel.
        if !collecting || deps_truncated || malformed {
            continue;
        }
        let Some(dep) = parse_transition_row(&line) else {
            malformed = true;
            continue;
        };
        if deps.contains(&dep) {
            continue;
        }
        if deps.len() < MAX_CONFIGURED_DEPS {
            deps.push(dep);
        } else {
            deps_truncated = true;
        }
    }
    if malformed {
        return Err(format!("unsupported cquery transitions row under {label}"));
    }
    if !found {
        return Err(format!("cquery reported no configuration for {label}"));
    }
    Ok(TargetTransitions {
        deps,
        deps_truncated,
    })
}

/// A column-0 header `[<rule transition> -> ]<label> (<id>)` names `label`
/// under the selected configuration. Labels cannot contain whitespace, so
/// the segment after the last ` -> ` is the label; `<id>` is the
/// configuration's short checksum — a prefix of the full hash `config()`
/// reported — or `null` for targets without a configuration.
fn is_target_header(line: &str, label: &str, checksum: Option<&str>) -> bool {
    let Some((prefix, id)) = line
        .strip_suffix(')')
        .and_then(|line| line.rsplit_once(" ("))
    else {
        return false;
    };
    !id.is_empty()
        && prefix
            .rsplit(" -> ")
            .next()
            .is_some_and(|found| crate::label::normalize(found) == label)
        && checksum.map_or(id == "null", |sum| sum.starts_with(id))
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

    const TRANSITIONS_FIXTURE: &str = include_str!("../../tests/fixtures/cquery-transitions.txt");

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
    fn transitions_fixture_reads_only_the_selected_root() {
        // The fixture lists //app:tool and //app:input.txt headers before
        // //app:chosen; only the chosen header's own rows become deps.
        let parsed = parse_transitions(
            TRANSITIONS_FIXTURE.as_bytes(),
            "//app:chosen",
            Some("7f8856bdeadbeef"),
        )
        .unwrap();
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
    fn transitions_fixture_supports_tool_and_null_config_headers() {
        let tool = parse_transitions(
            TRANSITIONS_FIXTURE.as_bytes(),
            "//app:tool",
            Some("f50cc19deadbeef"),
        )
        .unwrap();
        assert_eq!(
            dep_pairs(&tool),
            [("@bazel_tools//tools/genrule:genrule-setup.sh", None)]
        );

        let source =
            parse_transitions(TRANSITIONS_FIXTURE.as_bytes(), "//app:input.txt", None).unwrap();
        assert!(source.deps.is_empty());
    }

    #[test]
    fn transitions_rejects_a_missing_root_header() {
        assert!(
            parse_transitions(TRANSITIONS_FIXTURE.as_bytes(), "//app:missing", Some("abc"))
                .is_err()
        );
        // A null-configuration target rejects a configured checksum too.
        assert!(
            parse_transitions(
                TRANSITIONS_FIXTURE.as_bytes(),
                "//app:input.txt",
                Some("abc")
            )
            .is_err()
        );
    }

    #[test]
    fn transitions_ignore_same_label_headers_under_other_configurations() {
        // deps(config(...)) scopes to one configuration, but retained
        // same-label headers can still appear; only the checksum-matched
        // header collects rows — wrong configurations before AND after.
        let input = "\
NoTransition -> //app:a (aaa1111)
  deps#//app:stale#NoTransition ->
NoTransition -> //app:b (bbb2222)
  deps#//app:other#NoTransition ->
NoTransition -> //app:a (2d89340)
  deps#//app:live#(exec + (Trim)) -> ddd4444
NoTransition -> //app:a (ccc3333)
  deps#//app:later#NoTransition ->
";
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("2d89340cafe")).unwrap();
        assert_eq!(dep_pairs(&parsed), [("//app:live", Some("exec"))]);
    }

    #[test]
    fn transitions_normalize_canonical_main_repo_labels() {
        // cquery may print the main repository as @@//; it means // here,
        // while @@repo stays canonical and distinct from @repo.
        let input = "\
NoTransition -> @@//app:a (aaa1111)
  deps#@@//app:x#NoTransition ->
  deps#@@rules_cc//cc:defs.bzl%defs#NoTransition ->
  deps#@repo//pkg:y#NoTransition ->
";
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        assert_eq!(
            dep_pairs(&parsed),
            [
                ("//app:x", None),
                ("@@rules_cc//cc:defs.bzl%defs", None),
                ("@repo//pkg:y", None),
            ]
        );
    }

    #[test]
    fn transitions_dedupe_rows_and_ignore_later_headers() {
        let input = "\
NoTransition -> //app:a (aaa1111)
  deps#//app:shared#(exec + (Trim)) -> bbb2222
  deps#//app:shared#(exec + (Trim)) -> bbb2222
  srcs#//app:one#NoTransition ->
  data#//app:two#(null transition) ->
NoTransition -> //app:b (bbb2222)
  deps#//app:other#(null transition) ->
NoTransition -> //app:a (aaa1111)
  deps#//app:stale#(null transition) ->
";
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        // Duplicate rows collapse; once collection ends no later header —
        // even an identical one — resumes it.
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
    fn transition_rows_keep_hashes_inside_labels() {
        let input = "NoTransition -> //app:a (aaa1111)\n  deps#//app:x#y#(null transition) ->\n";
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        assert_eq!(parsed.deps[0].label, "//app:x#y");
        assert_eq!(parsed.deps[0].transition, None);
    }

    #[test]
    fn transition_rows_split_starlark_transitions_before_the_last_hash() {
        // A Starlark transition path may contain '#'; the label may too.
        // The #(Starlark transition: boundary — not the last '#' — splits.
        let input = "\
NoTransition -> //app:a (aaa1111)
  deps#//pkg:x#y#(Starlark transition:/ws/rules#v2.bzl:4:40 + (TestTrimmingTransition)) ->
";
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        assert_eq!(parsed.deps.len(), 1);
        assert_eq!(parsed.deps[0].label, "//pkg:x#y");
        assert_eq!(
            parsed.deps[0].transition.as_deref(),
            Some("(Starlark transition:/ws/rules#v2.bzl:4:40 + (TestTrimmingTransition))")
        );
    }

    #[test]
    fn transition_rows_reject_malformed_rows_but_skip_blanks() {
        let input = "\
NoTransition -> //app:a (aaa1111)

  deps#//app:x#NoTransition ->
  this is not a row
";
        let error = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap_err();
        assert!(
            error.contains("unsupported cquery transitions row"),
            "{error}"
        );
        // The diagnostic is bounded: it names the label, never the row.
        assert!(!error.contains("this is not a row"), "{error}");
    }

    #[test]
    fn transition_rows_skip_blank_lines_without_ending_the_section() {
        // A blank line is not a header: collection continues across it.
        let input = "\
NoTransition -> //app:a (aaa1111)

  deps#//app:x#NoTransition ->

  deps#//app:y#NoTransition ->
";
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        assert_eq!(dep_pairs(&parsed), [("//app:x", None), ("//app:y", None)]);
    }

    #[test]
    fn transition_rows_stop_at_the_dependency_cap() {
        let mut input = String::from("NoTransition -> //app:a (aaa1111)\n");
        for i in 0..MAX_CONFIGURED_DEPS + 5 {
            input.push_str(&format!("  deps#//app:d{i}#NoTransition ->\n"));
        }
        let parsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        assert_eq!(parsed.deps.len(), MAX_CONFIGURED_DEPS);
        assert!(parsed.deps_truncated);
        // Rows past the cap are ignored entirely, duplicates included.
        input.push_str("  deps#//app:d0#NoTransition ->\n");
        let reparsed = parse_transitions(input.as_bytes(), "//app:a", Some("aaa1111")).unwrap();
        assert_eq!(reparsed.deps.len(), MAX_CONFIGURED_DEPS);
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

    #[test]
    fn config_summary_keeps_its_wire_shape_without_the_id() {
        let summary = ConfigSummary {
            id: 7,
            mnemonic: "k8-opt".to_string(),
            checksum: "abc123".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&summary).unwrap(),
            serde_json::json!({
                "mnemonic": "k8-opt",
                "checksum": "abc123",
            })
        );
    }
}
