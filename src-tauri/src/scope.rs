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

    /// Rules and source files needed to classify target relationships.
    pub fn query(&self) -> String {
        format!(
            "let selected = {} in $selected union kind(\"source file\", deps($selected, 1))",
            self.rules_query()
        )
    }
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
            scope.query(),
            "let selected = kind(\"rule\", (\"//a:all\" union \"//b/...\")) in $selected union kind(\"source file\", deps($selected, 1))"
        );
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
            scope.query(),
            "let selected = kind(\"rule\", (\"//pkg:thing!%\" union \"//a:b:c\" union \"//a) union //...\" union \"//pkg:it's\")) in $selected union kind(\"source file\", deps($selected, 1))"
        );
    }
}
