//! Bazel label helpers: normalization, package/name split, file path derivation.

/// Normalize absolute labels found in `bazel query --output=xml` output.
/// Keep canonical (`@@repo`) and apparent (`@repo`) repository names distinct:
/// converting between them requires the referring repository's mapping.
/// The main repository's `@@//` is equivalent to `//` in this query context.
/// `//pkg` expands to `//pkg:<last segment>`; likewise `@repo//pkg`.
/// Anything else is returned unchanged.
#[must_use]
pub fn normalize(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("@@") {
        return match rest.split_once("//") {
            Some(("", p)) => normalize_main(p),
            Some((repo, p)) => format!("@@{}", normalize_repo(repo, p)),
            None => raw.to_string(),
        };
    }
    if let Some(rest) = raw.strip_prefix('@') {
        if let Some((repo, p)) = rest.split_once("//") {
            return format!("@{}", normalize_repo(repo, p));
        }
        return raw.to_string();
    }
    if let Some(rest) = raw.strip_prefix("//") {
        return normalize_main(rest);
    }
    raw.to_string()
}

/// `pkg` or `pkg:name` after `//` -> fully qualified `//pkg:name`.
fn normalize_main(rest: &str) -> String {
    match rest.split_once(':') {
        Some(_) => format!("//{rest}"),
        None => format!("//{rest}:{}", last_segment(rest)),
    }
}

/// `pkg` or `pkg:name` after a repository prefix -> `repo//pkg:name`.
fn normalize_repo(repo: &str, p: &str) -> String {
    match p.split_once(':') {
        Some(_) => format!("{repo}//{p}"),
        None => format!("{repo}//{p}:{}", last_segment(p)),
    }
}

fn last_segment(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// The package and target name extracted from a label.
#[derive(Debug, PartialEq, Eq)]
pub struct LabelParts {
    pub package: String,
    pub name: String,
}

/// Split a label into its package and name. For `@repo//p:n` the package is `p`
/// (the repo part is dropped; the tree only spans the main repository).
#[must_use]
pub fn split(label: &str) -> LabelParts {
    let rest = match label.find("//") {
        Some(i) => &label[i + 2..],
        None => label,
    };
    match rest.split_once(':') {
        Some((p, n)) => LabelParts {
            package: p.to_string(),
            name: n.to_string(),
        },
        None => LabelParts {
            package: rest.to_string(),
            name: last_segment(rest).to_string(),
        },
    }
}

/// Workspace-relative path of a file label: `//pkg:name` -> `pkg/name`.
#[must_use]
pub fn label_to_path(label: &str) -> String {
    let LabelParts { package, name } = split(label);
    if package.is_empty() {
        name
    } else {
        format!("{package}/{name}")
    }
}

/// Whether the label points into an external repository (`@repo//...`).
#[must_use]
pub fn is_external(label: &str) -> bool {
    label.starts_with('@')
}

/// Whether the label is Bazel's reserved default `select()` branch.
/// Package names alone cannot distinguish conditions from ordinary inputs.
#[must_use]
pub fn is_select_key(label: &str) -> bool {
    label == "//conditions:default"
}

/// Whether a label-valued attribute expresses a dependency edge.
/// `visibility` is access control and names starting with `$` or `:` are
/// implicit (late-bound) attributes; neither is a dependency.
#[must_use]
pub fn is_dep_attr(attr: &str) -> bool {
    !attr.is_empty() && !attr.starts_with('$') && !attr.starts_with(':') && attr != "visibility"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_main_repository_labels() {
        assert_eq!(normalize("//app"), "//app:app");
        assert_eq!(normalize("//app/lib"), "//app/lib:lib");
        assert_eq!(normalize("//app:lib"), "//app:lib");
        assert_eq!(normalize("//:root"), "//:root");
    }

    #[test]
    fn preserves_repository_name_kind() {
        assert_eq!(normalize("@@//app:x"), "//app:x");
        assert_eq!(normalize("@@repo//p:n"), "@@repo//p:n");
        assert_eq!(normalize("@@repo+//p"), "@@repo+//p:p");
        assert_eq!(normalize("@repo//p:n"), "@repo//p:n");
        assert_eq!(normalize("@@repo"), "@@repo");
        assert_eq!(normalize("@maven//:guava"), "@maven//:guava");
    }

    #[test]
    fn splits_labels() {
        assert_eq!(
            split("//app/lib:parser"),
            LabelParts {
                package: "app/lib".into(),
                name: "parser".into(),
            }
        );
        assert_eq!(
            split("//:root"),
            LabelParts {
                package: "".into(),
                name: "root".into(),
            }
        );
        assert_eq!(
            split("@maven//:guava"),
            LabelParts {
                package: "".into(),
                name: "guava".into(),
            }
        );
    }

    #[test]
    fn maps_labels_to_paths() {
        assert_eq!(
            label_to_path("//app/lib:Parser.java"),
            "app/lib/Parser.java"
        );
        assert_eq!(label_to_path("//:README.md"), "README.md");
    }

    #[test]
    fn classifies_dependency_attributes() {
        assert!(is_dep_attr("deps"));
        assert!(!is_dep_attr("visibility"));
        assert!(!is_dep_attr("$cc_toolchain"));
        assert!(!is_dep_attr(":late_bound"));
    }

    #[test]
    fn recognizes_the_default_select_key() {
        assert!(is_select_key("//conditions:default"));
        assert!(!is_select_key("//tools/conditions:arm"));
        assert!(!is_select_key("//conditions:inputs"));
        assert!(!is_select_key("//conditions:input.txt"));
        assert!(!is_select_key("//app:server"));
    }
}
