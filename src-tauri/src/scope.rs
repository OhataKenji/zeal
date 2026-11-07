//! Explicit target patterns used to bound workspace discovery.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Scope {
    pub include: Vec<String>,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            include: vec!["//...".to_string()],
        }
    }
}

impl Scope {
    /// Validate and normalize the patterns at the IPC and persistence boundaries.
    pub fn validated(self) -> Result<Self, String> {
        let include = normalize(self.include)?;
        if include.is_empty() {
            return Err("include at least one target pattern".to_string());
        }
        Ok(Self { include })
    }

    /// Rule labels and kinds needed by the target tree.
    pub fn rules_query(&self) -> String {
        let selected = self
            .include
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(" union ");
        format!("kind(\"rule\", ({selected}))")
    }
}

/// The selected rule and its direct source files, needed to classify its attributes.
pub fn target_query(label: &str) -> Result<String, String> {
    validate_label(label)?;
    Ok(format!(
        "let selected = \"{label}\" in $selected union kind(\"source file\", deps($selected, 1))"
    ))
}

/// The target pattern for `bazel cquery`, validated like [`target_query`].
pub fn cquery_target(label: &str) -> Result<String, String> {
    validate_label(label)?;
    Ok(label.to_string())
}

fn validate_label(label: &str) -> Result<(), String> {
    if !label.starts_with("//")
        || label
            .chars()
            .any(|c| c == '"' || c == '\\' || c.is_control())
    {
        return Err("invalid target label".to_string());
    }
    Ok(())
}

fn normalize(patterns: Vec<String>) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for raw in patterns {
        let pattern = raw.trim();
        if pattern.is_empty() {
            continue;
        }
        if !pattern.starts_with("//")
            || pattern
                .chars()
                .any(|c| c == '"' || c == '\\' || c.is_control())
        {
            return Err("target patterns must start with // and contain no double quotes, backslashes, or control characters".to_string());
        }
        normalized.push(pattern.to_string());
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_drops_blank_patterns_preserving_order_and_duplicates() {
        let scope = Scope {
            include: vec![
                " //b/... ".into(),
                "".into(),
                "//a:all".into(),
                "//b/...".into(),
            ],
        }
        .validated()
        .unwrap();
        assert_eq!(scope.include, ["//b/...", "//a:all", "//b/..."]);
    }

    #[test]
    fn multiple_patterns_are_combined_as_a_union() {
        let scope = Scope {
            include: vec!["//a:all".into(), "//b/...".into()],
        }
        .validated()
        .unwrap();
        assert_eq!(
            scope.rules_query(),
            "kind(\"rule\", (\"//a:all\" union \"//b/...\"))"
        );
    }

    #[test]
    fn default_is_whole_workspace_but_explicit_empty_scope_is_rejected() {
        assert_eq!(Scope::default().validated().unwrap().include, ["//..."]);
        assert!(
            Scope {
                include: vec![" ".into()]
            }
            .validated()
            .is_err()
        );
    }

    #[test]
    fn boundary_rejects_unsafe_tokens_and_nonabsolute_patterns() {
        for pattern in [
            "//a\"",
            "//a\\b",
            "//a\nb",
            "//a\tb",
            "//a\0b",
            "@repo//a/...",
            "a/...",
        ] {
            assert!(
                Scope {
                    include: vec![pattern.into()]
                }
                .validated()
                .is_err(),
                "{pattern:?}"
            );
        }
    }

    #[test]
    fn leaves_pattern_grammar_to_bazel_and_keeps_each_input_in_one_token() {
        let scope = Scope {
            include: vec![
                "//pkg:thing!%".into(),
                "//a:b:c".into(),
                "//a) union //...".into(),
                "//pkg:it's".into(),
            ],
        }
        .validated()
        .unwrap();
        assert_eq!(
            scope.rules_query(),
            "kind(\"rule\", (\"//pkg:thing!%\" union \"//a:b:c\" union \"//a) union //...\" union \"//pkg:it's\"))"
        )
    }

    #[test]
    fn cquery_target_validates_like_target_query() {
        assert_eq!(cquery_target("//app:server").unwrap(), "//app:server");
        assert!(cquery_target("app:server").is_err());
        assert!(cquery_target("//app:\"quoted\"").is_err());
    }

    #[test]
    fn target_query_selects_one_rule_and_its_direct_source_files() {
        assert_eq!(
            target_query("//app:chosen").unwrap(),
            "let selected = \"//app:chosen\" in $selected union kind(\"source file\", deps($selected, 1))"
        );
    }

    #[test]
    fn target_query_rejects_unsafe_labels() {
        for label in ["//app:\"x", "//app:\\x", "//app:\nx", "@repo//app:x"] {
            assert!(target_query(label).is_err(), "{label:?}");
        }
    }
}
