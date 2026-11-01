//! Explicit target patterns used to bound workspace discovery.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Scope {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
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

    /// All tokens are validated and quoted; patterns cannot introduce operators.
    pub fn query(&self) -> Result<String, String> {
        let scope = self.clone().validated()?;
        let includes = union(&scope.include);
        let selected = if scope.exclude.is_empty() {
            includes
        } else {
            format!("({includes}) except ({})", union(&scope.exclude))
        };
        Ok(format!(
            "let selected = kind(\"rule\", ({selected})) in $selected union kind(\"source file\", deps($selected, 1))"
        ))
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
        if !valid_pattern(pattern) {
            return Err(format!(
                "invalid target pattern: {pattern}; use //package/..., //package:all, or //package:target"
            ));
        }
        normalized.push(pattern.to_string());
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn valid_pattern(pattern: &str) -> bool {
    let Some(rest) = pattern.strip_prefix("//") else {
        return false;
    };
    if !rest
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"_-/.:+*=,@~".contains(&c))
    {
        return false;
    }
    if rest == "..." {
        return true;
    }
    if let Some(package) = rest.strip_suffix("/...") {
        return valid_path(package) && !package.contains(':') && !package.contains('*');
    }
    match rest.split_once(':') {
        Some((package, target)) => {
            (package.is_empty() || valid_path(package))
                && !package.contains('*')
                && (target == "*" || valid_path(target))
                && !target.contains(':')
                && (target == "*" || !target.contains('*'))
        }
        None => valid_path(rest) && !rest.contains('*'),
    }
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".." && p != "...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_patterns_are_combined_before_exclusions() {
        let scope = Scope {
            include: vec![" //b/... ".into(), "//a:all".into(), "//b/...".into()],
            exclude: vec!["//b/old/...".into()],
        }
        .validated()
        .unwrap();
        assert_eq!(scope.include, ["//a:all", "//b/..."]);
        assert_eq!(
            scope.query().unwrap(),
            "let selected = kind(\"rule\", ((\"//a:all\" union \"//b/...\") except (\"//b/old/...\"))) in $selected union kind(\"source file\", deps($selected, 1))"
        );
    }

    #[test]
    fn empty_scope_never_means_the_whole_workspace() {
        assert!(Scope::default().query().is_err());
        assert!(
            Scope {
                include: vec![" ".into()],
                exclude: vec![]
            }
            .validated()
            .is_err()
        );
        assert!(valid_pattern("//..."));
        assert!(valid_pattern("//:all"));
        assert!(valid_pattern("//pkg:target+variant"));
    }

    #[test]
    fn rejects_query_injection_relative_and_malformed_patterns() {
        for pattern in [
            "//a) union //...",
            "//a\"",
            "@repo//a/...",
            "a/...",
            "//a/../b:all",
            "//a//b:all",
            "//a:",
            "//a:b:c",
            "//a/*",
        ] {
            assert!(!valid_pattern(pattern), "{pattern}");
        }
    }
}
