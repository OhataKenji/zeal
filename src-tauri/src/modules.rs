//! Bzlmod `bazel mod` views: the resolved module graph and per-module MVS
//! explanations. Commands run bounded; `MODULE.bazel` must already exist so
//! Bazel never creates one, and `--lockfile_mode=off` keeps
//! `MODULE.bazel.lock` untouched.

use crate::bazel_output;
use crate::state::Core;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Module resolution and extension evaluation can fetch dependencies on a
/// cold cache. Keep a deadline that allows large workspaces to finish.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(180);
const GRAPH_STDOUT_CAP: usize = 16 * 1024 * 1024;
const EXPLAIN_STDOUT_CAP: usize = 2 * 1024 * 1024;
const EXPLAIN_TRUNCATION_NOTE: &str = "\n\n… (output truncated at 2 MiB)";
const MAX_GRAPH_DEPTH: usize = 60;
const MAX_MODULES: usize = 2000;
const MAX_EDGES: usize = 20_000;
/// Every graph key — root included, even one seen only as an edge endpoint —
/// is cloned into the output, so unbounded keys would amplify the input
/// without limit.
const MAX_KEY_LEN: usize = 256;

const GRAPH_ARGS: &[&str] = &[
    "mod",
    "graph",
    "--color=no",
    "--noshow_timestamps",
    "--lockfile_mode=off",
    "--output=json",
    "--depth=2147483647",
    "--from=<root>",
    "--noinclude_unused",
    "--extension_info=hidden",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleNode {
    pub key: String,
    pub name: String,
    pub version: String,
    /// `true` if this module is a direct dependency of the root module.
    pub direct: bool,
    /// Parent module keys, sorted and deduplicated; may contain `root_key`.
    pub required_by: Vec<String>,
    /// Child module keys, sorted and deduplicated.
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModulesSnapshot {
    pub root_key: String,
    pub root_label: String,
    pub modules: Vec<ModuleNode>,
    pub warning: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModuleExplanation {
    pub text: String,
    pub warning: Option<String>,
}

/// Fail-fast guard owning one workspace's `bazel mod` operations: the
/// registered path, configured binary, shared shutdown signal and the busy
/// mark are taken under a single state lock. Dropping the guard — at the end
/// of the blocking closure — releases the mark and wakes exit. There is no
/// wait queue.
pub struct ModulesOperation {
    core: Arc<Core>,
    bazel: PathBuf,
    workspace: PathBuf,
    cancelled: Arc<AtomicBool>,
}

impl ModulesOperation {
    /// Errors when the app is shutting down, `workspace` is not registered,
    /// no bazel is configured, or another modules operation is already
    /// running for it.
    pub fn acquire(core: &Arc<Core>, workspace: &str) -> Result<Self, String> {
        let mut st = core.state.lock().expect("application state lock poisoned");
        if st.shutting_down.load(Ordering::Relaxed) {
            return Err("application is shutting down".to_string());
        }
        let workspace_path = st.workspace_path(workspace)?;
        let bazel = st.bazel.clone().ok_or_else(|| "no bazel".to_string())?;
        if !st.modules_running.insert(workspace_path.clone()) {
            return Err("a modules operation is already running for this workspace".to_string());
        }
        Ok(Self {
            core: core.clone(),
            bazel,
            workspace: workspace_path,
            cancelled: st.shutting_down.clone(),
        })
    }

    /// The resolved module graph.
    pub fn snapshot(&self) -> Result<ModulesSnapshot, String> {
        let out = self.run(GRAPH_ARGS, GRAPH_STDOUT_CAP)?;
        if out.stdout_truncated {
            return Err("bazel mod graph output exceeded 16 MiB".into());
        }
        let mut snapshot = parse_mod_graph(&out.stdout)?;
        snapshot.warning = out.failure();
        Ok(snapshot)
    }

    /// `bazel mod explain --verbose` output for one module key.
    pub fn explain(&self, module_key: &str) -> Result<ModuleExplanation, String> {
        validate_module_key(module_key)?;
        // `explain` defaults to a shallow view that hides transitive modules;
        // the depth flag matches `mod graph` so leaves and MVS causes appear.
        let args = [
            "mod",
            "explain",
            "--color=no",
            "--noshow_timestamps",
            "--lockfile_mode=off",
            "--output=text",
            "--from=<root>",
            "--verbose",
            "--noinclude_unused",
            "--depth=2147483647",
            "--",
            module_key,
        ];
        let out = self.run(&args, EXPLAIN_STDOUT_CAP)?;
        Ok(ModuleExplanation {
            text: explain_output(&out.stdout, out.stdout_truncated),
            warning: out.failure(),
        })
    }

    /// One bounded Bazel call. `MODULE.bazel` is re-checked before every
    /// spawn so Bazel can never recreate it.
    fn run(&self, args: &[&str], cap: usize) -> Result<bazel_output::BoundedOutput, String> {
        require_module_bazel(&self.workspace)?;
        let out = bazel_output::run_bazel_bounded(
            &self.bazel,
            &self.workspace,
            args,
            Instant::now() + OPERATION_TIMEOUT,
            cap,
            &self.cancelled,
        )?;
        // Bazel can write a result before reporting extension inspection failures.
        let partial = out.status.code() == Some(2)
            && out.stderr().lines().any(|line| {
                line.starts_with("ERROR: Results may be incomplete as ")
                    && (line.ends_with(" extension failed.")
                        || line.ends_with(" extensions failed."))
            })
            && !String::from_utf8_lossy(&out.stdout).trim().is_empty();
        if partial {
            Ok(out)
        } else {
            out.require_success()
        }
    }
}

impl Drop for ModulesOperation {
    fn drop(&mut self) {
        let mut st = self
            .core
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        st.modules_running.remove(&self.workspace);
        self.core.invocation_finished.notify_all();
    }
}

/// Bazel creates an empty `MODULE.bazel` when a `mod` command runs without
/// one; refuse to launch so workspaces stay untouched.
fn require_module_bazel(ws: &Path) -> Result<(), String> {
    if ws.join("MODULE.bazel").is_file() {
        Ok(())
    } else {
        Err("MODULE.bazel not found in workspace".to_string())
    }
}

/// `mod explain` output as plain text. A capped stream can end mid-codepoint;
/// only complete UTF-8 is kept and the truncation is announced.
fn explain_output(stdout: &[u8], truncated: bool) -> String {
    if !truncated {
        return String::from_utf8_lossy(stdout).into_owned();
    }
    // An incomplete trailing sequence reports `valid_up_to`; other invalid
    // bytes keep the full length so lossy decoding handles them.
    let end = match std::str::from_utf8(stdout) {
        Ok(_) => stdout.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => stdout.len(),
    };
    let mut text = String::from_utf8_lossy(&stdout[..end]).into_owned();
    text.push_str(EXPLAIN_TRUNCATION_NOTE);
    text
}

/// One node of `bazel mod graph --output=json`. Unknown fields (`apparentName`,
/// verbose extras, `extensionUsages`) are ignored; `indirectDependencies` is
/// always empty under the fixed arguments used here.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNode {
    key: String,
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    dependencies: Vec<RawNode>,
    #[serde(default)]
    cycles: Vec<RawNode>,
    #[serde(default)]
    unexpanded: bool,
}

/// Record `node` under the graph caps and merge its explicit metadata — a
/// repeated occurrence may supply fields an earlier stub lacked, while
/// missing fields never overwrite known ones.
fn observe_node<'a>(
    nodes: &mut BTreeMap<&'a str, ModuleNode>,
    node: &'a RawNode,
) -> Result<(), String> {
    let key = node.key.as_str();
    if key.len() > MAX_KEY_LEN {
        return Err(format!("bazel mod graph key exceeds {MAX_KEY_LEN} bytes"));
    }
    let module = nodes.entry(key).or_insert_with(|| {
        let (name, version) = key.split_once('@').unwrap_or((key, ""));
        ModuleNode {
            key: key.into(),
            name: name.into(),
            version: normalize_version(version),
            direct: false,
            required_by: Vec::new(),
            dependencies: Vec::new(),
        }
    });
    if let Some(name) = &node.name {
        module.name.clone_from(name);
    }
    if let Some(version) = &node.version {
        module.version = normalize_version(version);
    }
    // The root occupies one slot but is excluded only at projection.
    if nodes.len() > MAX_MODULES + 1 {
        return Err(format!("bazel mod graph exceeds {MAX_MODULES} modules"));
    }
    Ok(())
}

fn normalize_version(version: &str) -> String {
    if version == "_" {
        String::new()
    } else {
        version.to_string()
    }
}

/// Fold the collected relation into the snapshot. Sorted unique edges make
/// both adjacency lists sorted for free; the root was removed above, so its
/// edge endpoints resolve to `None` and are skipped.
fn project(
    mut nodes: BTreeMap<&str, ModuleNode>,
    mut edges: Vec<(&str, &str)>,
    root_key: &str,
    root_label: String,
) -> ModulesSnapshot {
    nodes.remove(root_key);
    edges.sort_unstable();
    edges.dedup();
    for (parent, child) in edges {
        if let Some(module) = nodes.get_mut(parent) {
            module.dependencies.push(child.into());
        }
        if let Some(module) = nodes.get_mut(child) {
            module.required_by.push(parent.into());
            module.direct |= parent == root_key;
        }
    }
    let mut modules: Vec<ModuleNode> = nodes.into_values().collect();
    modules.sort_by(|a, b| {
        b.direct
            .cmp(&a.direct)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.version.cmp(&b.version))
            .then_with(|| a.key.cmp(&b.key))
    });
    ModulesSnapshot {
        root_key: root_key.to_string(),
        root_label,
        modules,
        warning: None,
    }
}

/// Parse `bazel mod graph --output=json` into a snapshot. Collection walks
/// the tree once — every observed node merges metadata and every raw
/// parent→child occurrence records an edge — then projection folds the
/// edge relation into adjacency lists.
fn parse_mod_graph(stdout: &[u8]) -> Result<ModulesSnapshot, String> {
    let root: RawNode =
        serde_json::from_slice(stdout).map_err(|e| format!("invalid bazel mod graph JSON: {e}"))?;
    let mut nodes = BTreeMap::new();
    let mut edges = Vec::new();
    let mut expanded = BTreeSet::new();
    // (node, depth, expand): cycle-sourced occurrences are observed for
    // metadata but their subtrees are never walked.
    let mut pending = vec![(&root, 0usize, true)];
    while let Some((node, depth, expand)) = pending.pop() {
        observe_node(&mut nodes, node)?;
        if !expand || node.unexpanded || !expanded.insert(node.key.as_str()) {
            continue;
        }
        if depth > MAX_GRAPH_DEPTH {
            return Err(format!("bazel mod graph exceeds depth {MAX_GRAPH_DEPTH}"));
        }
        let children = node.dependencies.len() + node.cycles.len();
        if edges.len() + children > MAX_EDGES {
            return Err(format!("bazel mod graph exceeds {MAX_EDGES} edges"));
        }
        // Push reversed so the stack pops dependencies before cycles, each
        // in listed order — the preorder the recursive walk used.
        for child in node.cycles.iter().rev() {
            edges.push((node.key.as_str(), child.key.as_str()));
            pending.push((child, depth + 1, false));
        }
        for child in node.dependencies.iter().rev() {
            edges.push((node.key.as_str(), child.key.as_str()));
            pending.push((child, depth + 1, true));
        }
    }
    let root_label = match root.name.as_deref() {
        Some(name) if !name.is_empty() => {
            let version = root
                .version
                .as_deref()
                .filter(|v| !v.is_empty())
                .unwrap_or("_");
            format!("{} ({name}@{version})", root.key)
        }
        _ => root.key.clone(),
    };
    Ok(project(nodes, edges, &root.key, root_label))
}

/// Module keys travel to `bazel mod explain` as a single argv element after
/// `--`; restrict them to characters that can never look like a flag.
fn validate_module_key(module_key: &str) -> Result<(), String> {
    let valid = !module_key.is_empty()
        && module_key.len() <= MAX_KEY_LEN
        && !module_key.starts_with('-')
        && module_key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '@' | '~' | '-'));
    if valid {
        Ok(())
    } else {
        Err("invalid module key".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::test_support::{TempDir, fake_bazel};

    /// Real `bazel mod graph --output=json` captured with `--noinclude_unused`
    /// from the Bzlmod diamond fixture: `mod_a@_`/`mod_b@_` local overrides
    /// share `mod_c@3.0.0` and the MVS winner `mod_e@2.0.0`.
    const GRAPH_JSON: &[u8] = include_bytes!("../tests/fixtures/modules-graph.json");

    fn by_key(modules: &[ModuleNode]) -> BTreeMap<&str, &ModuleNode> {
        modules.iter().map(|m| (m.key.as_str(), m)).collect()
    }

    /// The committed real-Bazel capture, parsed fresh per test.
    fn fixture() -> ModulesSnapshot {
        parse_mod_graph(GRAPH_JSON).unwrap()
    }

    /// Core with `ws` registered and `bazel` configured — the single state
    /// owner every acquired-operation test builds on.
    fn core_with(ws: &Path, bazel: &Path) -> Arc<Core> {
        let core = Arc::new(Core::new());
        let mut st = core.state.lock().unwrap();
        st.workspaces
            .insert(ws.to_path_buf(), crate::scope::Scope::default());
        st.bazel = Some(bazel.to_path_buf());
        drop(st);
        core
    }

    #[test]
    fn parse_mod_graph_folds_captured_bazel_output() {
        let snap = fixture();
        assert_eq!(snap.root_key, "<root>");
        assert_eq!(snap.root_label, "<root> (zelith_bzlmod_fixture@0.0.1)");
        // Direct dependencies first, then name order.
        let keys: Vec<&str> = snap.modules.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["mod_a@_", "mod_b@_", "mod_c@3.0.0", "mod_e@2.0.0"]);
        let by_key = by_key(&snap.modules);
        assert!(by_key["mod_a@_"].direct && by_key["mod_b@_"].direct);
        assert!(!by_key["mod_c@3.0.0"].direct && !by_key["mod_e@2.0.0"].direct);
        assert_eq!(by_key["mod_a@_"].version, "");
        assert_eq!(by_key["mod_c@3.0.0"].version, "3.0.0");
    }

    #[test]
    fn parse_mod_graph_merges_diamond_edges_across_stubs() {
        let snap = fixture();
        let by_key = by_key(&snap.modules);
        // The diamond merges edges recorded under the expanded occurrence
        // (mod_a) and the `unexpanded` stubs (mod_b).
        for key in ["mod_c@3.0.0", "mod_e@2.0.0"] {
            assert_eq!(by_key[key].required_by, ["mod_a@_", "mod_b@_"], "{key}");
            assert!(by_key[key].dependencies.is_empty(), "{key}");
        }
        for key in ["mod_a@_", "mod_b@_"] {
            assert_eq!(
                by_key[key].dependencies,
                ["mod_c@3.0.0", "mod_e@2.0.0"],
                "{key}"
            );
            assert_eq!(by_key[key].required_by, ["<root>"], "{key}");
        }
    }

    #[test]
    fn parse_mod_graph_falls_back_to_key_parts_and_normalizes_underscore() {
        let json = r#"{"key":"<root>","dependencies":[
            {"key":"odd@9.9","dependencies":[]},
            {"key":"under@_","name":"under","version":"_","dependencies":[]}
        ]}"#;
        let snap = parse_mod_graph(json.as_bytes()).unwrap();
        assert_eq!(snap.root_label, "<root>");
        let modules = snap.modules;
        assert_eq!(modules.len(), 2);
        let odd = modules.iter().find(|m| m.key == "odd@9.9").unwrap();
        assert_eq!(odd.name, "odd");
        assert_eq!(odd.version, "9.9");
        let under = modules.iter().find(|m| m.key == "under@_").unwrap();
        assert_eq!(under.version, "");
    }

    #[test]
    fn parse_mod_graph_labels_root_with_fallback_version() {
        // A named root without a usable version keeps Bazel's `name@_` shape.
        for json in [
            r#"{"key":"<root>","name":"demo","dependencies":[]}"#,
            r#"{"key":"<root>","name":"demo","version":"","dependencies":[]}"#,
        ] {
            let snap = parse_mod_graph(json.as_bytes()).unwrap();
            assert_eq!(snap.root_label, "<root> (demo@_)");
        }
    }

    #[test]
    fn parse_mod_graph_keeps_metadata_a_stub_lacked() {
        // The `unexpanded` stub is observed first; the later expanded
        // occurrence must still supply the version its key fallback lacks.
        let json = r#"{"key":"<root>","dependencies":[
            {"key":"a@1","dependencies":[
                {"key":"late@_","unexpanded":true}
            ]},
            {"key":"b@1","dependencies":[
                {"key":"late@_","name":"late","version":"2","dependencies":[]}
            ]}
        ]}"#;
        let modules = parse_mod_graph(json.as_bytes()).unwrap().modules;
        let late = modules.iter().find(|m| m.key == "late@_").unwrap();
        assert_eq!(late.name, "late");
        assert_eq!(late.version, "2");
        assert_eq!(late.required_by, ["a@1", "b@1"]);
    }

    #[test]
    fn parse_mod_graph_records_cycle_edges_without_recursing() {
        // Bzlmod forbids dependency cycles, but a `cycles` entry still
        // contributes its edge in both directions without re-walking `a@1`.
        let json = r#"{"key":"<root>","dependencies":[
            {"key":"a@1","dependencies":[
                {"key":"b@1","dependencies":[],
                 "cycles":[{"key":"a@1","unexpanded":true}]}
            ]}
        ]}"#;
        let modules = parse_mod_graph(json.as_bytes()).unwrap().modules;
        assert_eq!(modules.len(), 2);
        let by_key = by_key(&modules);
        assert_eq!(by_key["a@1"].required_by, ["<root>", "b@1"]);
        assert_eq!(by_key["b@1"].required_by, ["a@1"]);
        assert_eq!(by_key["b@1"].dependencies, ["a@1"]);
    }

    #[test]
    fn parse_mod_graph_survives_back_edge_stubs() {
        // A stub pointing back at an expanded ancestor must not recurse or
        // lose the edge.
        let json = r#"{"key":"<root>","dependencies":[
            {"key":"a@1","dependencies":[
                {"key":"b@1","dependencies":[
                    {"key":"a@1","unexpanded":true}
                ]}
            ]}
        ]}"#;
        let modules = parse_mod_graph(json.as_bytes()).unwrap().modules;
        let a = modules.iter().find(|m| m.key == "a@1").unwrap();
        assert_eq!(a.required_by, ["<root>", "b@1"]);
        let b = modules.iter().find(|m| m.key == "b@1").unwrap();
        assert_eq!(b.required_by, ["a@1"]);
        assert_eq!(b.dependencies, ["a@1"]);
    }

    #[test]
    fn parse_mod_graph_rejects_invalid_json() {
        for input in [b"".as_slice(), b"not json", b"{", b"{\"dependencies\":[]}"] {
            assert!(parse_mod_graph(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn parse_mod_graph_rejects_oversized_keys() {
        // A huge key would be cloned into every edge list referencing it,
        // amplifying a small input without bound.
        let key = format!("k{}@1", "x".repeat(MAX_KEY_LEN));
        let json = format!(
            "{{\"key\":\"<root>\",\"dependencies\":[{{\"key\":\"{key}\",\"dependencies\":[]}}]}}"
        );
        assert!(parse_mod_graph(json.as_bytes()).is_err());
        // The root key is guarded too.
        let json = format!("{{\"key\":\"{key}\",\"dependencies\":[]}}");
        assert!(parse_mod_graph(json.as_bytes()).is_err());
    }

    fn deep_graph(levels: usize) -> String {
        let mut json = String::from("{\"key\":\"<root>\",\"dependencies\":[");
        for i in (1..=levels).rev() {
            json.push_str(&format!("{{\"key\":\"n{i}@1\",\"dependencies\":["));
        }
        json.push_str("{\"key\":\"leaf@1\",\"dependencies\":[]}");
        for _ in 0..=levels {
            json.push_str("]}");
        }
        json
    }

    #[test]
    fn parse_mod_graph_enforces_depth_node_and_edge_caps() {
        // leaf sits at depth `levels + 1`; 59 stays inside the cap of 60.
        assert!(parse_mod_graph(deep_graph(59).as_bytes()).is_ok());
        assert!(parse_mod_graph(deep_graph(60).as_bytes()).is_err());

        let children: Vec<serde_json::Value> = (0..=MAX_MODULES)
            .map(|i| serde_json::json!({"key": format!("m{i}@1"), "dependencies": []}))
            .collect();
        let json = serde_json::json!({"key": "<root>", "dependencies": children});
        assert!(parse_mod_graph(&serde_json::to_vec(&json).unwrap()).is_err());

        // 100 parents x 200 shared stubs = 20 100 recorded edges.
        let stubs: Vec<serde_json::Value> = (0..200)
            .map(|i| serde_json::json!({"key": format!("s{i}@1"), "unexpanded": true}))
            .collect();
        let parents: Vec<serde_json::Value> = (0..100)
            .map(|i| serde_json::json!({"key": format!("p{i}@1"), "dependencies": stubs}))
            .collect();
        let json = serde_json::json!({"key": "<root>", "dependencies": parents});
        assert!(parse_mod_graph(&serde_json::to_vec(&json).unwrap()).is_err());
    }

    #[test]
    fn explain_output_marks_truncation_at_a_utf8_boundary() {
        assert_eq!(explain_output(b"plain text", false), "plain text");
        // The capped stream ends with the first two bytes of '€' (3 bytes).
        let mut bytes = vec![b'a'; EXPLAIN_STDOUT_CAP - 2];
        bytes.extend_from_slice(&[0xE2, 0x82]);
        let text = explain_output(&bytes, true);
        assert!(text.ends_with(EXPLAIN_TRUNCATION_NOTE));
        let kept = text.strip_suffix(EXPLAIN_TRUNCATION_NOTE).unwrap();
        assert!(kept.chars().all(|c| c == 'a'), "split UTF-8 leaked");
        assert_eq!(kept.len(), EXPLAIN_STDOUT_CAP - 2);
    }

    #[test]
    fn validate_module_key_accepts_real_keys() {
        let longest = "x".repeat(256);
        for key in [
            "rules_cc@0.0.17",
            "name@_",
            "a.b+c~d-e_f@g",
            longest.as_str(),
        ] {
            assert!(validate_module_key(key).is_ok(), "{key}");
        }
    }

    #[test]
    fn validate_module_key_rejects_unsafe_keys() {
        let longest = "x".repeat(257);
        for key in [
            "",
            "-flag",
            "has space",
            "has\nnewline",
            "has/slash",
            "has:colon",
            "unicodé",
            longest.as_str(),
        ] {
            assert!(validate_module_key(key).is_err(), "{key:?}");
        }
    }

    /// Poll until `path` exists or `within` elapses; fails the test
    /// otherwise. Used to prove a fake bazel subprocess is genuinely running.
    fn wait_for_file(path: &Path, within: Duration) {
        let give_up = Instant::now() + within;
        while !path.exists() {
            assert!(
                Instant::now() < give_up,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    mod process {
        use super::*;
        use std::fs;
        use std::time::Duration;

        /// An acquired operation with `dir` itself as the workspace: an
        /// empty `MODULE.bazel` exists and `bazel` is the fake script, whose
        /// cwd is the workspace so relative paths land beside it.
        fn operation(dir: &TempDir, script: &str) -> ModulesOperation {
            fs::write(dir.path().join("MODULE.bazel"), "").unwrap();
            let bazel = fake_bazel(dir.path(), "bazel", script);
            ModulesOperation::acquire(&core_with(dir.path(), &bazel), dir.path().to_str().unwrap())
                .unwrap()
        }

        #[test]
        fn snapshot_runs_only_mod_graph() {
            let dir = TempDir::new();
            fs::write(dir.path().join("graph.json"), GRAPH_JSON).unwrap();
            let op = operation(&dir, "#!/bin/sh\necho \"$3\" >> calls\ncat graph.json\n");
            let snapshot = op.snapshot().unwrap();
            assert_eq!(snapshot.root_key, "<root>");
            assert_eq!(snapshot.modules.len(), 4);
            assert_eq!(
                fs::read_to_string(dir.path().join("calls")).unwrap(),
                "graph\n"
            );
        }

        #[test]
        fn snapshot_fails_when_graph_fails() {
            let dir = TempDir::new();
            let op = operation(&dir, "#!/bin/sh\nexit 1\n");
            let err = op.snapshot().unwrap_err();
            assert!(err.contains("mod graph"), "{err}");
        }

        #[test]
        fn snapshot_keeps_bazels_partial_graph_with_the_extension_error() {
            let dir = TempDir::new();
            fs::write(dir.path().join("graph.json"), GRAPH_JSON).unwrap();
            // A workspace `.bazelrc` with `common --show_timestamps` prefixes
            // stderr event lines like `(14:32:05) ERROR: ...`; the flag keeps
            // them bare so the partial-graph notice is still recognized.
            let op = operation(
                &dir,
                "#!/bin/sh\ncase \" $* \" in *\" --noshow_timestamps \"*) ;; *) ts='(14:32:05) ' ;; esac\n\
                 cat graph.json\n\
                 echo \"${ts}ERROR: missing generated repository\" >&2\n\
                 echo \"${ts}ERROR: Results may be incomplete as 1 extension failed.\" >&2\nexit 2\n",
            );
            let snapshot = op.snapshot().unwrap();
            assert_eq!(snapshot.modules, fixture().modules);
            assert!(
                snapshot
                    .warning
                    .unwrap()
                    .contains("missing generated repository")
            );
        }

        #[test]
        fn snapshot_rejects_other_failed_commands_even_if_they_emit_graph_json() {
            let dir = TempDir::new();
            fs::write(dir.path().join("graph.json"), GRAPH_JSON).unwrap();
            let op = operation(
                &dir,
                "#!/bin/sh\ncat graph.json\necho 'ERROR: invalid argument' >&2\nexit 2\n",
            );
            assert!(op.snapshot().unwrap_err().contains("invalid argument"));
        }

        #[test]
        fn modules_commands_refuse_workspaces_without_module_bazel() {
            let dir = TempDir::new();
            let ws = dir.path().join("ws");
            fs::create_dir_all(&ws).unwrap();
            let bazel = fake_bazel(dir.path(), "bazel", "#!/bin/sh\ntouch invoked\n");
            let core = core_with(&ws, &bazel);
            let op = ModulesOperation::acquire(&core, ws.to_str().unwrap()).unwrap();
            assert_eq!(
                op.snapshot().unwrap_err(),
                "MODULE.bazel not found in workspace"
            );
            assert_eq!(
                op.explain("mod_a@_").unwrap_err(),
                "MODULE.bazel not found in workspace"
            );
            assert!(!ws.join("invoked").exists(), "bazel must not be spawned");
        }

        #[test]
        fn explain_returns_text_and_rejects_bad_keys_first() {
            let dir = TempDir::new();
            let op = operation(
                &dir,
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > args\necho 'mod_a@_ was selected'\n",
            );
            let explanation = op.explain("mod_a@_").unwrap();
            assert_eq!(explanation.text, "mod_a@_ was selected\n");
            assert_eq!(explanation.warning, None);
            let args = fs::read_to_string(dir.path().join("args")).unwrap();
            let lines: Vec<&str> = args.lines().collect();
            assert!(lines.contains(&"--color=no") && lines.contains(&"--noshow_timestamps"));
            assert!(
                ["--depth=2147483647", "--noinclude_unused", "--verbose"]
                    .iter()
                    .all(|flag| lines.contains(flag)),
                "{args}"
            );
            // The module key is the last argv element, after `--`.
            assert_eq!(lines.last(), Some(&"mod_a@_"), "{args}");
            // Validation happens before bazel is even spawned.
            let err = op.explain("-bad").unwrap_err();
            assert!(err.contains("module key"), "{err}");
        }

        #[test]
        fn explain_keeps_bazels_partial_output_with_the_extension_error() {
            let dir = TempDir::new();
            let op = operation(
                &dir,
                "#!/bin/sh\necho 'mod_a@_ was selected'\n\
                 echo 'ERROR: missing generated repository' >&2\n\
                 echo 'ERROR: Results may be incomplete as 2 extensions failed.' >&2\nexit 2\n",
            );
            let explanation = op.explain("mod_a@_").unwrap();
            assert_eq!(explanation.text, "mod_a@_ was selected\n");
            assert!(
                explanation
                    .warning
                    .unwrap()
                    .contains("missing generated repository")
            );
        }

        #[test]
        fn explain_rejects_extension_failure_without_an_explanation() {
            let dir = TempDir::new();
            let op = operation(
                &dir,
                "#!/bin/sh\necho '  '\n\
                 echo 'ERROR: Results may be incomplete as 1 extension failed.' >&2\nexit 2\n",
            );
            let error = op.explain("mod_a@_").unwrap_err();
            assert!(error.contains("1 extension failed"), "{error}");
        }

        #[test]
        fn exit_stop_cancels_an_in_flight_modules_operation() {
            let dir = TempDir::new();
            let op = operation(&dir, "#!/bin/sh\ntouch started\nsleep 30\n");
            // The worker needs the core for exit_stop; clone it before moving op.
            let core = op.core.clone();
            let worker = std::thread::spawn(move || op.snapshot());
            // Wait until the subprocess is actually running before exit.
            wait_for_file(&dir.path().join("started"), Duration::from_secs(10));
            let started = Instant::now();
            crate::runner::exit_stop(&core);
            assert!(started.elapsed() < Duration::from_secs(5));
            assert!(core.state.lock().unwrap().modules_running.is_empty());
            let err = worker.join().unwrap().unwrap_err();
            assert!(err.contains("shutting down"), "{err}");
        }
    }

    mod guard {
        use super::*;

        #[test]
        fn acquire_fails_fast_per_workspace_and_releases_on_drop() {
            let ws = PathBuf::from("/ws/modules-test");
            let bazel = PathBuf::from("/bin/true");
            let core = core_with(&ws, &bazel);
            let other = PathBuf::from("/ws/other");
            let mut st = core.state.lock().unwrap();
            st.workspaces
                .insert(other.clone(), crate::scope::Scope::default());
            drop(st);
            let op = ModulesOperation::acquire(&core, ws.to_str().unwrap()).unwrap();
            assert_eq!(op.workspace, ws);
            assert_eq!(op.bazel, bazel);
            // Same workspace fails fast; a different one is unaffected.
            assert!(ModulesOperation::acquire(&core, ws.to_str().unwrap()).is_err());
            let other_op = ModulesOperation::acquire(&core, other.to_str().unwrap());
            assert!(other_op.is_ok());
            drop(other_op);
            // Releasing the guard frees the workspace again.
            drop(op);
            assert!(ModulesOperation::acquire(&core, ws.to_str().unwrap()).is_ok());
        }

        #[test]
        fn acquire_rejects_unregistered_workspace_and_missing_bazel() {
            let ws = PathBuf::from("/ws/modules-test");
            let core = core_with(&ws, Path::new("/bin/true"));
            assert!(ModulesOperation::acquire(&core, "/ws/nope").is_err());
            let mut st = core.state.lock().unwrap();
            st.bazel = None;
            drop(st);
            assert!(ModulesOperation::acquire(&core, ws.to_str().unwrap()).is_err());
        }

        #[test]
        fn acquire_rejects_new_operations_once_shutdown_begins() {
            let ws = PathBuf::from("/ws/modules-test");
            let core = core_with(&ws, Path::new("/bin/true"));
            crate::runner::exit_stop(&core);
            let err = ModulesOperation::acquire(&core, ws.to_str().unwrap())
                .err()
                .unwrap();
            assert_eq!(err, "application is shutting down");
        }
    }
}
