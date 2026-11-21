//! Bzlmod `bazel mod` views: the resolved module graph and per-module MVS
//! explanations. Every command is read-only and bounded; `MODULE.bazel` must
//! already exist so Bazel never creates one, and `--lockfile_mode=off` keeps
//! `MODULE.bazel.lock` untouched.

use crate::bazel_output;
use crate::state::Core;
use serde::{Deserialize, Serialize};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Per-Bazel-call bound for `bazel mod` commands.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const GRAPH_STDOUT_CAP: usize = 16 * 1024 * 1024;
const EXPLAIN_STDOUT_CAP: usize = 2 * 1024 * 1024;
const EXPLAIN_TRUNCATION_NOTE: &str = "\n\n… (output truncated at 2 MiB)";
const MAX_GRAPH_DEPTH: usize = 60;
const MAX_MODULES: usize = 2000;
const MAX_EDGES: usize = 20_000;
/// Every graph key — root included, even one only seen inside `children` or
/// `cycles` — is cloned into edge lists, so unbounded keys would amplify the
/// input without limit.
const MAX_KEY_LEN: usize = 256;

const GRAPH_ARGS: &[&str] = &[
    "mod",
    "graph",
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
    pub modules: Vec<ModuleNode>,
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
        let (root_key, modules) = parse_mod_graph(&out.stdout)?;
        Ok(ModulesSnapshot { root_key, modules })
    }

    /// `bazel mod explain --verbose` output for one module key.
    pub fn explain(&self, module_key: &str) -> Result<String, String> {
        validate_module_key(module_key)?;
        // `explain` defaults to a shallow view that hides transitive modules;
        // the depth flag matches `mod graph` so leaves and MVS causes appear.
        let args = [
            "mod",
            "explain",
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
        Ok(explain_output(&out.stdout, out.stdout_truncated))
    }

    /// One bounded Bazel call. `MODULE.bazel` is re-checked before every
    /// spawn so Bazel can never recreate it.
    fn run(&self, args: &[&str], cap: usize) -> Result<bazel_output::BoundedOutput, String> {
        require_module_bazel(&self.workspace)?;
        bazel_output::run_bazel_bounded(
            &self.bazel,
            &self.workspace,
            args,
            Instant::now() + OPERATION_TIMEOUT,
            cap,
            &self.cancelled,
        )
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
    let end = utf8_complete_len(stdout);
    let mut text = String::from_utf8_lossy(&stdout[..end]).into_owned();
    text.push_str(EXPLAIN_TRUNCATION_NOTE);
    text
}

/// Byte length of `bytes` when it ends on a UTF-8 boundary. An incomplete
/// trailing sequence reports `valid_up_to`; other invalid bytes keep the full
/// length so lossy decoding handles them.
fn utf8_complete_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => bytes.len(),
    }
}

/// One node of `bazel mod graph --output=json`. Unknown fields (`apparentName`,
/// verbose extras, `extensionUsages`) are ignored; `indirectDependencies` is
/// always empty under the fixed arguments used here.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNode {
    key: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    dependencies: Vec<RawNode>,
    #[serde(default)]
    cycles: Vec<RawNode>,
    #[serde(default)]
    unexpanded: bool,
}

struct Fold {
    root_key: String,
    nodes: BTreeMap<String, ModuleNode>,
    expanded: BTreeSet<String>,
    edges: usize,
}

impl Fold {
    /// Insert on first observation with key-part fallbacks, or merge
    /// non-`None` `name`/`version` when a later occurrence supplies them:
    /// an `unexpanded` stub can precede the expanded node and lack either.
    fn ensure(&mut self, node: &RawNode) -> Result<(), String> {
        if node.key.len() > MAX_KEY_LEN {
            return Err(format!("bazel mod graph key exceeds {MAX_KEY_LEN} bytes"));
        }
        // The root occupies one slot but is not counted as a module.
        if !self.nodes.contains_key(&node.key)
            && node.key != self.root_key
            && self.nodes.len() > MAX_MODULES
        {
            return Err(format!("bazel mod graph exceeds {MAX_MODULES} modules"));
        }
        let acc = match self.nodes.entry(node.key.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let (key_name, key_version) = key_parts(&node.key);
                entry.insert(ModuleNode {
                    key: node.key.clone(),
                    name: node.name.clone().unwrap_or(key_name),
                    version: normalize_version(node.version.clone().unwrap_or(key_version)),
                    direct: false,
                    required_by: Vec::new(),
                    dependencies: Vec::new(),
                })
            }
        };
        // A repeated occurrence may supply `name`/`version` a stub lacked;
        // applying it to a fresh insert is a no-op.
        if let Some(name) = &node.name {
            acc.name.clone_from(name);
        }
        if let Some(version) = &node.version {
            acc.version = normalize_version(version.clone());
        }
        Ok(())
    }

    /// Record the parent→child edge in both directions, once per observed
    /// edge. `unexpanded` stubs and `cycles` entries contribute theirs even
    /// though their subtrees are never walked.
    fn edge(&mut self, parent: &RawNode, child: &RawNode) -> Result<(), String> {
        self.edges += 1;
        if self.edges > MAX_EDGES {
            return Err(format!("bazel mod graph exceeds {MAX_EDGES} edges"));
        }
        // Both endpoints have already been visited.
        if parent.key != self.root_key
            && let Some(acc) = self.nodes.get_mut(&parent.key)
        {
            acc.dependencies.push(child.key.clone());
        }
        if let Some(acc) = self.nodes.get_mut(&child.key) {
            acc.required_by.push(parent.key.clone());
        }
        Ok(())
    }

    /// Ensure `node`, record its edge to `parent` when there is one, then
    /// walk its children unless it is an `unexpanded` stub or was already
    /// expanded — `expanded` doubles as the recursion guard since ancestors
    /// are inserted before their children are walked.
    fn visit(
        &mut self,
        node: &RawNode,
        parent: Option<&RawNode>,
        depth: usize,
    ) -> Result<(), String> {
        self.ensure(node)?;
        if let Some(parent) = parent {
            self.edge(parent, node)?;
        }
        if node.unexpanded || !self.expanded.insert(node.key.clone()) {
            return Ok(());
        }
        if depth > MAX_GRAPH_DEPTH {
            return Err(format!("bazel mod graph exceeds depth {MAX_GRAPH_DEPTH}"));
        }
        for child in &node.dependencies {
            self.visit(child, Some(node), depth + 1)?;
        }
        for child in &node.cycles {
            // Cycle edges are recorded but their targets are never expanded.
            self.ensure(child)?;
            self.edge(node, child)?;
        }
        Ok(())
    }
}

/// Fold the `bazel mod graph --output=json` tree into deduplicated module
/// records, returning `(root_key, modules)` with the root excluded. A
/// module is `direct` when the root key appears in its `required_by`.
fn parse_mod_graph(stdout: &[u8]) -> Result<(String, Vec<ModuleNode>), String> {
    let root: RawNode =
        serde_json::from_slice(stdout).map_err(|e| format!("invalid bazel mod graph JSON: {e}"))?;
    let mut fold = Fold {
        root_key: root.key.clone(),
        nodes: BTreeMap::new(),
        expanded: BTreeSet::new(),
        edges: 0,
    };
    fold.visit(&root, None, 0)?;
    let Fold {
        root_key, nodes, ..
    } = fold;
    let mut modules: Vec<ModuleNode> = nodes.into_values().filter(|m| m.key != root_key).collect();
    for module in &mut modules {
        module.required_by.sort_unstable();
        module.required_by.dedup();
        module.dependencies.sort_unstable();
        module.dependencies.dedup();
        module.direct = module.required_by.contains(&root_key);
    }
    modules.sort_by(|a, b| {
        b.direct
            .cmp(&a.direct)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.version.cmp(&b.version))
            .then_with(|| a.key.cmp(&b.key))
    });
    Ok((root_key, modules))
}

/// `name@version` split for nodes lacking explicit fields; a key without
/// `@` is its own name with an empty version.
fn key_parts(key: &str) -> (String, String) {
    match key.split_once('@') {
        Some((name, version)) => (name.to_string(), version.to_string()),
        None => (key.to_string(), String::new()),
    }
}

fn normalize_version(version: String) -> String {
    if version == "_" {
        String::new()
    } else {
        version
    }
}

/// Module keys travel to `bazel mod explain` as a single argv element after
/// `--`; restrict them to characters that can never look like a flag.
fn validate_module_key(module_key: &str) -> Result<(), String> {
    if module_key.is_empty() {
        return Err("module key is empty".to_string());
    }
    if module_key.len() > 256 {
        return Err("module key is too long".to_string());
    }
    if module_key.starts_with('-') {
        return Err("module key must not start with '-'".to_string());
    }
    if !module_key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '@' | '~' | '-'))
    {
        return Err("module key contains invalid characters".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::test_support::{TempDir, fake_bazel};
    use std::fs;

    /// Real `bazel mod graph --output=json` captured with `--noinclude_unused`
    /// from the Bzlmod diamond fixture: `mod_a@_`/`mod_b@_` local overrides
    /// share `mod_c@3.0.0` and the MVS winner `mod_e@2.0.0`.
    const GRAPH_JSON: &[u8] = include_bytes!("../tests/fixtures/modules-graph.json");

    fn ws_with_module(dir: &TempDir) -> PathBuf {
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("MODULE.bazel"), "module(name = \"ws\")\n").unwrap();
        ws
    }

    fn by_key(modules: &[ModuleNode]) -> BTreeMap<&str, &ModuleNode> {
        modules.iter().map(|m| (m.key.as_str(), m)).collect()
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
        let (root_key, modules) = parse_mod_graph(GRAPH_JSON).unwrap();
        assert_eq!(root_key, "<root>");
        // Direct dependencies first, then name order.
        let keys: Vec<&str> = modules.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["mod_a@_", "mod_b@_", "mod_c@3.0.0", "mod_e@2.0.0"]);
        let by_key = by_key(&modules);
        assert!(by_key["mod_a@_"].direct && by_key["mod_b@_"].direct);
        assert!(!by_key["mod_c@3.0.0"].direct && !by_key["mod_e@2.0.0"].direct);
        assert_eq!(by_key["mod_a@_"].version, "");
        assert_eq!(by_key["mod_c@3.0.0"].version, "3.0.0");
    }

    #[test]
    fn parse_mod_graph_merges_diamond_edges_across_stubs() {
        let (_, modules) = parse_mod_graph(GRAPH_JSON).unwrap();
        let by_key = by_key(&modules);
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
        let (_, modules) = parse_mod_graph(json.as_bytes()).unwrap();
        assert_eq!(modules.len(), 2);
        let odd = modules.iter().find(|m| m.key == "odd@9.9").unwrap();
        assert_eq!(odd.name, "odd");
        assert_eq!(odd.version, "9.9");
        let under = modules.iter().find(|m| m.key == "under@_").unwrap();
        assert_eq!(under.version, "");
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
        let (_, modules) = parse_mod_graph(json.as_bytes()).unwrap();
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
        let (_, modules) = parse_mod_graph(json.as_bytes()).unwrap();
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
        let (_, modules) = parse_mod_graph(json.as_bytes()).unwrap();
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

        let children: Vec<String> = (0..=MAX_MODULES)
            .map(|i| format!("{{\"key\":\"m{i}@1\",\"dependencies\":[]}}"))
            .collect();
        let json = format!(
            "{{\"key\":\"<root>\",\"dependencies\":[{}]}}",
            children.join(",")
        );
        assert!(parse_mod_graph(json.as_bytes()).is_err());

        // 100 parents x 200 shared stubs = 20 100 recorded edges.
        let deps: Vec<String> = (0..200)
            .map(|i| format!("{{\"key\":\"s{i}@1\",\"unexpanded\":true}}"))
            .collect();
        let deps = deps.join(",");
        let parents: Vec<String> = (0..100)
            .map(|i| format!("{{\"key\":\"p{i}@1\",\"dependencies\":[{deps}]}}"))
            .collect();
        let json = format!(
            "{{\"key\":\"<root>\",\"dependencies\":[{}]}}",
            parents.join(",")
        );
        assert!(parse_mod_graph(json.as_bytes()).is_err());
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

        /// Fake bazel that records each invoked subcommand — argv is
        /// `--noblock_for_lock mod <sub>` …, so `$3` — and serves the graph
        /// fixture for any call.
        fn bazel_serving_graph(dir: &TempDir) -> (PathBuf, PathBuf) {
            let graph_path = dir.path().join("graph.json");
            fs::write(&graph_path, GRAPH_JSON).unwrap();
            let calls = dir.path().join("calls.txt");
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"$3\" >> '{}'\ncat '{}'\n",
                calls.display(),
                graph_path.display()
            );
            (fake_bazel(dir.path(), "bazel", &script), calls)
        }

        #[test]
        fn snapshot_runs_only_mod_graph() {
            let dir = TempDir::new();
            let ws = ws_with_module(&dir);
            let (bazel, calls) = bazel_serving_graph(&dir);
            let core = core_with(&ws, &bazel);
            let op = ModulesOperation::acquire(&core, ws.to_str().unwrap()).unwrap();
            let snapshot = op.snapshot().unwrap();
            assert_eq!(snapshot.root_key, "<root>");
            assert_eq!(snapshot.modules.len(), 4);
            assert_eq!(fs::read_to_string(&calls).unwrap(), "graph\n");
        }

        #[test]
        fn snapshot_fails_when_graph_fails() {
            let dir = TempDir::new();
            let ws = ws_with_module(&dir);
            let bazel = fake_bazel(dir.path(), "bazel", "#!/bin/sh\nexit 1\n");
            let core = core_with(&ws, &bazel);
            let op = ModulesOperation::acquire(&core, ws.to_str().unwrap()).unwrap();
            let err = op.snapshot().unwrap_err();
            assert!(err.contains("mod graph"), "{err}");
        }

        #[test]
        fn modules_commands_refuse_workspaces_without_module_bazel() {
            let dir = TempDir::new();
            let ws = dir.path().join("ws");
            fs::create_dir_all(&ws).unwrap();
            let core = core_with(&ws, Path::new("/nonexistent/zeal-bazel"));
            let op = ModulesOperation::acquire(&core, ws.to_str().unwrap()).unwrap();
            assert_eq!(
                op.snapshot().unwrap_err(),
                "MODULE.bazel not found in workspace"
            );
            assert_eq!(
                op.explain("mod_a@_").unwrap_err(),
                "MODULE.bazel not found in workspace"
            );
        }

        #[test]
        fn explain_returns_text_and_rejects_bad_keys_first() {
            let dir = TempDir::new();
            let ws = ws_with_module(&dir);
            fs::write(dir.path().join("explain.txt"), "mod_a@_ was selected\n").unwrap();
            let args_path = dir.path().join("args.txt");
            let script = format!(
                "#!/bin/sh\nprintf '%s' \"$*\" > '{}'\ncat '{}'\n",
                args_path.display(),
                dir.path().join("explain.txt").display()
            );
            let bazel = fake_bazel(dir.path(), "bazel", &script);
            let core = core_with(&ws, &bazel);
            let op = ModulesOperation::acquire(&core, ws.to_str().unwrap()).unwrap();
            assert_eq!(op.explain("mod_a@_").unwrap(), "mod_a@_ was selected\n");
            let args = fs::read_to_string(&args_path).unwrap();
            assert!(args.contains("--depth=2147483647"), "{args}");
            assert!(args.contains("--noinclude_unused"), "{args}");
            // Validation happens before bazel is even spawned.
            let err = op.explain("-bad").unwrap_err();
            assert!(err.contains("module key"), "{err}");
        }

        #[test]
        fn exit_stop_cancels_an_in_flight_modules_operation() {
            let dir = TempDir::new();
            let ws = ws_with_module(&dir);
            // The fake bazel announces startup, then blocks: shutdown lands
            // while `mod graph` is genuinely in flight.
            let marker = dir.path().join("bazel-started");
            let bazel = fake_bazel(
                dir.path(),
                "bazel",
                &format!("#!/bin/sh\ntouch '{}'\nsleep 30\n", marker.display()),
            );
            let core = core_with(&ws, &bazel);
            let op = ModulesOperation::acquire(&core, ws.to_str().unwrap()).unwrap();
            let worker = std::thread::spawn(move || op.snapshot());
            // Wait until the subprocess is actually running before exit.
            wait_for_file(&marker, Duration::from_secs(10));
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
