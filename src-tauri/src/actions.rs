//! Parse of `bazel aquery --output=proto` output (`analysis_v2.proto`).
//!
//! The graph references artifacts and targets by numeric ids, so the parser
//! resolves them into display-ready rows: mnemonic, owning target label, and
//! primary output path.

use crate::protocol::analysis as proto;
use prost::Message;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;

/// One action reduced to the fields the target page lists.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionEntry {
    pub mnemonic: String,
    /// Label of the target responsible for the action.
    pub owner: String,
    /// Exec-root-relative path of the primary output, if any.
    pub output: String,
}

#[tracing::instrument(target = "zeal::operations", name = "aquery.proto_parse", skip_all)]
pub fn parse_actions(mut input: impl Read) -> Result<Vec<ActionEntry>, String> {
    let mut bytes = Vec::new();
    input
        .read_to_end(&mut bytes)
        .map_err(|e| format!("failed to read aquery output: {e}"))?;
    let graph = proto::ActionGraphContainer::decode(bytes.as_slice())
        .map_err(|e| format!("invalid aquery protobuf: {e}"))?;
    Ok(summarize(&graph))
}

fn summarize(graph: &proto::ActionGraphContainer) -> Vec<ActionEntry> {
    let mut fragments = FragmentPaths::new(&graph.path_fragments);
    let artifacts: HashMap<u32, &proto::Artifact> = graph
        .artifacts
        .iter()
        .map(|artifact| (artifact.id, artifact))
        .collect();
    let targets: HashMap<u32, &str> = graph
        .targets
        .iter()
        .map(|target| (target.id, target.label.as_str()))
        .collect();

    let mut entries = Vec::with_capacity(graph.actions.len());
    for action in &graph.actions {
        let artifact = (action.primary_output_id != 0)
            .then(|| artifacts.get(&action.primary_output_id))
            .flatten()
            .or_else(|| action.output_ids.first().and_then(|id| artifacts.get(id)));
        let output = artifact
            .and_then(|artifact| fragments.path(artifact.path_fragment_id))
            .map(str::to_string)
            .unwrap_or_default();
        entries.push(ActionEntry {
            mnemonic: action.mnemonic.clone(),
            owner: targets
                .get(&action.target_id)
                .map(|label| (*label).to_string())
                .unwrap_or_default(),
            output,
        });
    }
    entries.sort_by(|a, b| {
        (&a.owner, &a.mnemonic, &a.output).cmp(&(&b.owner, &b.mnemonic, &b.output))
    });
    entries
}

/// Exec paths are stored as a linked list of path fragments; resolve each id
/// once and cache the joined path.
struct FragmentPaths<'a> {
    fragments: HashMap<u32, &'a proto::PathFragment>,
    cache: HashMap<u32, String>,
}

impl<'a> FragmentPaths<'a> {
    fn new(fragments: &'a [proto::PathFragment]) -> Self {
        Self {
            fragments: fragments.iter().map(|f| (f.id, f)).collect(),
            cache: HashMap::new(),
        }
    }

    fn path(&mut self, id: u32) -> Option<&str> {
        if self.cache.contains_key(&id) {
            return self.cache.get(&id).map(String::as_str);
        }
        let mut segments = Vec::new();
        let mut cursor = self.fragments.get(&id).copied();
        loop {
            let fragment = cursor?;
            segments.push(fragment.label.as_str());
            cursor = match fragment.parent_id {
                0 => break,
                parent => self.fragments.get(&parent).copied(),
            };
        }
        segments.reverse();
        // The exec-root fragment has an empty label; it must not lead the path.
        if segments.first() == Some(&"") {
            segments.remove(0);
        }
        let path = segments.join("/");
        self.cache.insert(id, path);
        self.cache.get(&id).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(id: u32, label: &str, parent_id: u32) -> proto::PathFragment {
        proto::PathFragment {
            id,
            label: label.to_string(),
            parent_id,
        }
    }

    fn artifact(id: u32, fragment_id: u32) -> proto::Artifact {
        proto::Artifact {
            id,
            path_fragment_id: fragment_id,
            is_tree_artifact: false,
        }
    }

    #[test]
    fn resolves_owner_mnemonic_and_output() {
        let graph = proto::ActionGraphContainer {
            path_fragments: vec![
                fragment(1, "", 0),
                fragment(2, "bazel-out", 1),
                fragment(3, "bin", 2),
                fragment(4, "app", 3),
                fragment(5, "main.o", 4),
            ],
            artifacts: vec![artifact(1, 5), artifact(2, 5)],
            targets: vec![proto::Target {
                id: 1,
                label: "//app:server".to_string(),
                rule_class_id: 0,
            }],
            actions: vec![proto::Action {
                target_id: 1,
                mnemonic: "CppCompile".to_string(),
                primary_output_id: 1,
                output_ids: vec![1, 2],
                ..Default::default()
            }],
            ..Default::default()
        };
        let entries = summarize(&graph);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.owner, "//app:server");
        assert_eq!(entry.mnemonic, "CppCompile");
        assert_eq!(entry.output, "bazel-out/bin/app/main.o");
    }

    #[test]
    fn missing_references_degrade_to_empty_fields() {
        let graph = proto::ActionGraphContainer {
            actions: vec![proto::Action {
                mnemonic: "Symlink".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let entries = summarize(&graph);
        assert_eq!(entries[0].owner, "");
        assert_eq!(entries[0].output, "");
    }

    #[test]
    fn rejects_malformed_bytes() {
        assert!(parse_actions(&[0x0a, 0x80][..]).is_err());
    }
}
