//! Bazel label helpers: canonicalization, package/name split, file path derivation.

/// Canonical form for labels found in `bazel query --output=xml` output.
/// The XML always carries absolute labels. `@@x//` collapses to `@x//` (an
/// approximate apparent-name normalization; without a repo mapping this is the
/// most stable form to compare and display) and `@@//` to `//`.
/// `//pkg` expands to `//pkg:<last segment>`; likewise `@repo//pkg`.
/// Anything else is returned unchanged.
pub fn canonical(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("@@") {
        return match rest.split_once("//") {
            Some(("", p)) => canonical_main(p),
            Some((repo, p)) => format!("@{}", canonical_repo(repo, p)),
            None => format!("@{rest}"),
        };
    }
    if let Some(rest) = raw.strip_prefix('@') {
        if let Some((repo, p)) = rest.split_once("//") {
            return format!("@{}", canonical_repo(repo, p));
        }
        return raw.to_string();
    }
    if let Some(rest) = raw.strip_prefix("//") {
        return canonical_main(rest);
    }
    raw.to_string()
}

/// `pkg` or `pkg:name` after `//` -> canonical `//pkg:name`.
fn canonical_main(rest: &str) -> String {
    match rest.split_once(':') {
        Some(_) => format!("//{rest}"),
        None => format!("//{rest}:{}", last_segment(rest)),
    }
}

/// `pkg` or `pkg:name` after `@repo//` -> canonical `repo//pkg:name`.
fn canonical_repo(repo: &str, p: &str) -> String {
    match p.split_once(':') {
        Some(_) => format!("{repo}//{p}"),
        None => format!("{repo}//{p}:{}", last_segment(p)),
    }
}

fn last_segment(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Split a label into (package, name). For `@repo//p:n` the package is `p`
/// (the repo part is dropped; the tree only spans the main repository).
pub fn split(label: &str) -> (String, String) {
    let rest = match label.find("//") {
        Some(i) => &label[i + 2..],
        None => label,
    };
    match rest.split_once(':') {
        Some((p, n)) => (p.to_string(), n.to_string()),
        None => (rest.to_string(), last_segment(rest).to_string()),
    }
}

/// Workspace-relative path of a file label: `//pkg:name` -> `pkg/name`.
pub fn label_to_path(label: &str) -> String {
    let (pkg, name) = split(label);
    if pkg.is_empty() {
        name
    } else {
        format!("{pkg}/{name}")
    }
}

/// Whether the label points into an external repository (`@repo//...`).
pub fn is_external(label: &str) -> bool {
    label.starts_with('@')
}

/// Whether the label looks like a `select()` branch key
/// (`//conditions:default`, `//tools/conditions:arm` etc.).
pub fn is_select_key(label: &str) -> bool {
    if is_external(label) {
        return false;
    }
    let (pkg, _) = split(label);
    pkg == "conditions" || pkg.ends_with("/conditions")
}

/// Whether a label-valued attribute expresses a dependency edge.
/// `visibility` is access control and names starting with `$` or `:` are
/// implicit (late-bound) attributes; neither is a dependency.
pub fn is_dep_attr(attr: &str) -> bool {
    !attr.is_empty() && !attr.starts_with('$') && !attr.starts_with(':') && attr != "visibility"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_labels() {
        assert_eq!(canonical("//app"), "//app:app");
        assert_eq!(canonical("//app/lib"), "//app/lib:lib");
        assert_eq!(canonical("//app:lib"), "//app:lib");
        assert_eq!(canonical("//:root"), "//:root");
        assert_eq!(canonical("@@//app:x"), "//app:x");
        assert_eq!(canonical("@@repo//p:n"), "@repo//p:n");
        assert_eq!(canonical("@@repo+//p"), "@repo+//p:p");
        assert_eq!(canonical("@maven//:guava"), "@maven//:guava");
    }

    #[test]
    fn splits_labels() {
        assert_eq!(
            split("//app/lib:parser"),
            ("app/lib".into(), "parser".into())
        );
        assert_eq!(split("//:root"), ("".into(), "root".into()));
        assert_eq!(split("@maven//:guava"), ("".into(), "guava".into()));
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
    fn classifies_attributes_and_keys() {
        assert!(is_dep_attr("deps"));
        assert!(!is_dep_attr("visibility"));
        assert!(!is_dep_attr("$cc_toolchain"));
        assert!(!is_dep_attr(":late_bound"));
        assert!(is_select_key("//conditions:default"));
        assert!(is_select_key("//tools/conditions:arm"));
        assert!(!is_select_key("//app:server"));
    }
}
