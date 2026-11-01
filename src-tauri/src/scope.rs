//! Explicit target patterns used to bound workspace discovery.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Scope {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            include: vec!["//...".to_string()],
            exclude: Vec::new(),
        }
    }
}

impl Scope {
    /// Validate and normalize the patterns at the IPC and persistence boundaries.
    pub fn validated(self) -> Result<Self, String> {
        let include = normalize(self.include)?;
        let exclude = normalize(self.exclude)?;
        if include.is_empty() {
            return Err("include at least one target pattern".to_string());
        }
        Ok(Self { include, exclude })
    }

    /// Quote the scope accepted at the IPC or persistence boundary.
    pub fn query(&self) -> String {
        let includes = union(&self.include);
        let selected = if self.exclude.is_empty() {
            includes
        } else {
            format!("({includes}) except ({})", union(&self.exclude))
        };
        format!(
            "let selected = kind(\"rule\", ({selected})) in $selected union kind(\"source file\", deps($selected, 1))"
        )
    }
}

fn union(patterns: &[String]) -> String {
    patterns
        .iter()
        .map(|p| format!("\"{p}\""))
        .collect::<Vec<_>>()
        .join(" union ")
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
    fn normalizes_both_lists_at_the_boundary() {
        let scope = Scope {
            include: vec![
                " //b/... ".into(),
                "".into(),
                "//a:all".into(),
                "//b/...".into(),
            ],
            exclude: vec![
                " //b/old/... ".into(),
                " ".into(),
                "//a:skip".into(),
                "//b/old/...".into(),
            ],
        }
        .validated()
        .unwrap();
        assert_eq!(scope.include, ["//b/...", "//a:all", "//b/..."]);
        assert_eq!(scope.exclude, ["//b/old/...", "//a:skip", "//b/old/..."]);
    }

    #[test]
    fn multiple_patterns_are_combined_before_exclusions() {
        let scope = Scope {
            include: vec!["//a:all".into(), "//b/...".into()],
            exclude: vec!["//b/old/...".into()],
        }
        .validated()
        .unwrap();
        assert_eq!(
            scope.query(),
            "let selected = kind(\"rule\", ((\"//a:all\" union \"//b/...\") except (\"//b/old/...\"))) in $selected union kind(\"source file\", deps($selected, 1))"
        );
    }

    #[test]
    fn default_is_whole_workspace_but_explicit_empty_scope_is_rejected() {
        assert_eq!(Scope::default().validated().unwrap().include, ["//..."]);
        assert!(
            Scope {
                include: vec![" ".into()],
                exclude: vec![]
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
                    include: vec![pattern.into()],
                    exclude: vec![]
                }
                .validated()
                .is_err(),
                "{pattern:?}"
            );
            assert!(
                Scope {
                    include: vec!["//...".into()],
                    exclude: vec![pattern.into()]
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
            exclude: vec![],
        }
        .validated()
        .unwrap();
        assert_eq!(
            scope.query(),
            "let selected = kind(\"rule\", (\"//pkg:thing!%\" union \"//a:b:c\" union \"//a) union //...\" union \"//pkg:it's\")) in $selected union kind(\"source file\", deps($selected, 1))"
        );
    }
}
