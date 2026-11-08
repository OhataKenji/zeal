use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;

const MAX_ACTIONS: usize = 10;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetAction {
    pub output: String,
    pub mnemonic: String,
    pub owner: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetActions {
    pub actions: Vec<TargetAction>,
    pub truncated: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Graph {
    #[serde(default)]
    actions: Vec<ActionProto>,
    #[serde(default)]
    targets: Vec<TargetProto>,
    #[serde(default)]
    artifacts: Vec<ArtifactProto>,
    #[serde(default)]
    path_fragments: Vec<PathFragmentProto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionProto {
    target_id: u32,
    mnemonic: String,
    #[serde(default)]
    primary_output_id: u32,
    #[serde(default)]
    output_ids: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetProto {
    id: u32,
    label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactProto {
    id: u32,
    path_fragment_id: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathFragmentProto {
    id: u32,
    label: String,
    #[serde(default)]
    parent_id: u32,
}

pub fn parse(input: impl Read) -> Result<TargetActions, String> {
    let graph: Graph = serde_json::from_reader(input)
        .map_err(|error| format!("invalid aquery jsonproto: {error}"))?;
    let targets: HashMap<u32, String> = graph
        .targets
        .into_iter()
        .map(|target| (target.id, target.label))
        .collect();
    let artifacts: HashMap<u32, u32> = graph
        .artifacts
        .into_iter()
        .map(|artifact| (artifact.id, artifact.path_fragment_id))
        .collect();
    let fragments: HashMap<u32, PathFragmentProto> = graph
        .path_fragments
        .into_iter()
        .map(|fragment| (fragment.id, fragment))
        .collect();
    let truncated = graph.actions.len() > MAX_ACTIONS;
    let mut actions = Vec::with_capacity(graph.actions.len().min(MAX_ACTIONS));
    for action in graph.actions.iter().take(MAX_ACTIONS) {
        actions.push(summarize(action, &targets, &artifacts, &fragments)?);
    }
    Ok(TargetActions { actions, truncated })
}

fn summarize(
    action: &ActionProto,
    targets: &HashMap<u32, String>,
    artifacts: &HashMap<u32, u32>,
    fragments: &HashMap<u32, PathFragmentProto>,
) -> Result<TargetAction, String> {
    let owner = targets
        .get(&action.target_id)
        .ok_or_else(|| "aquery action references an unknown target".to_string())?;
    let output_id = if action.primary_output_id != 0 {
        action.primary_output_id
    } else {
        action.output_ids.first().copied().unwrap_or(0)
    };
    let output = match output_id {
        0 => "—".to_string(),
        id => artifact_path(id, artifacts, fragments)?,
    };
    Ok(TargetAction {
        output,
        mnemonic: action.mnemonic.clone(),
        owner: owner.clone(),
    })
}

fn artifact_path(
    artifact_id: u32,
    artifacts: &HashMap<u32, u32>,
    fragments: &HashMap<u32, PathFragmentProto>,
) -> Result<String, String> {
    let mut fragment_id = *artifacts
        .get(&artifact_id)
        .ok_or_else(|| "aquery action output is not a known artifact".to_string())?;
    let mut segments = Vec::new();
    let mut visited = HashSet::new();
    while fragment_id != 0 {
        if !visited.insert(fragment_id) {
            return Err("aquery path fragments contain a cycle".to_string());
        }
        let fragment = fragments
            .get(&fragment_id)
            .ok_or_else(|| "aquery artifact references an unknown path fragment".to_string())?;
        if !fragment.label.is_empty() {
            segments.push(fragment.label.as_str());
        }
        fragment_id = fragment.parent_id;
    }
    if segments.is_empty() {
        return Err("aquery artifact has an empty path".to_string());
    }
    segments.reverse();
    Ok(segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn container(actions: serde_json::Value) -> serde_json::Value {
        json!({
            "targets": [{"id": 1, "label": "//app:server", "ruleClass": 7}],
            "artifacts": [
                {"id": 10, "pathFragmentId": 105},
                {"id": 11, "pathFragmentId": 106},
            ],
            "pathFragments": [
                {"id": 100, "label": ""},
                {"id": 101, "label": "bazel-out", "parentId": 100},
                {"id": 102, "label": "k8-fastbuild", "parentId": 101},
                {"id": 103, "label": "bin", "parentId": 102},
                {"id": 104, "label": "app", "parentId": 103},
                {"id": 105, "label": "server_deploy.jar", "parentId": 104},
                {"id": 106, "label": "server", "parentId": 104},
            ],
            "actions": actions,
        })
    }

    fn parse_value(value: &serde_json::Value) -> Result<TargetActions, String> {
        parse(value.to_string().into_bytes().as_slice())
    }

    #[test]
    fn reconstructs_paths_owner_and_mnemonic() {
        let parsed = parse_value(&container(json!([
            {"targetId": 1, "mnemonic": "DeployJar", "outputIds": [10], "primaryOutputId": 10},
            {"targetId": 1, "mnemonic": "SingleJar", "outputIds": [10, 11], "primaryOutputId": 11},
        ])))
        .unwrap();
        assert!(!parsed.truncated);
        assert_eq!(parsed.actions.len(), 2);
        assert_eq!(
            parsed.actions[0].output,
            "bazel-out/k8-fastbuild/bin/app/server_deploy.jar"
        );
        assert_eq!(parsed.actions[0].mnemonic, "DeployJar");
        assert_eq!(parsed.actions[0].owner, "//app:server");
        assert_eq!(
            parsed.actions[1].output,
            "bazel-out/k8-fastbuild/bin/app/server"
        );
    }

    #[test]
    fn falls_back_to_the_first_output_and_dash_without_outputs() {
        let parsed = parse_value(&container(json!([
            {"targetId": 1, "mnemonic": "Javac", "outputIds": [10, 11]},
            {"targetId": 1, "mnemonic": "Symlink", "outputIds": []},
            {"targetId": 1, "mnemonic": "CopyFile"},
        ])))
        .unwrap();
        assert_eq!(
            parsed.actions[0].output,
            "bazel-out/k8-fastbuild/bin/app/server_deploy.jar"
        );
        assert_eq!(parsed.actions[1].output, "—");
        assert_eq!(parsed.actions[2].output, "—");
    }

    #[test]
    fn keeps_duplicate_output_rows_in_action_order() {
        let parsed = parse_value(&container(json!([
            {"targetId": 1, "mnemonic": "Javac", "outputIds": [10], "primaryOutputId": 10},
            {"targetId": 1, "mnemonic": "SourceJar", "outputIds": [10], "primaryOutputId": 10},
        ])))
        .unwrap();
        assert_eq!(parsed.actions.len(), 2);
        assert_eq!(parsed.actions[0].mnemonic, "Javac");
        assert_eq!(parsed.actions[1].mnemonic, "SourceJar");
        assert_eq!(parsed.actions[0].output, parsed.actions[1].output);
    }

    #[test]
    fn caps_retained_actions_at_ten() {
        let actions = (0..MAX_ACTIONS + 1)
            .map(|_| json!({"targetId": 1, "mnemonic": "Javac", "outputIds": [10]}))
            .collect::<Vec<_>>();
        let parsed = parse_value(&container(json!(actions))).unwrap();
        assert_eq!(parsed.actions.len(), MAX_ACTIONS);
        assert!(parsed.truncated);

        let actions = (0..MAX_ACTIONS)
            .map(|_| json!({"targetId": 1, "mnemonic": "Javac", "outputIds": [10]}))
            .collect::<Vec<_>>();
        let parsed = parse_value(&container(json!(actions))).unwrap();
        assert_eq!(parsed.actions.len(), MAX_ACTIONS);
        assert!(!parsed.truncated);

        let parsed = parse_value(&container(json!([]))).unwrap();
        assert!(parsed.actions.is_empty());
        assert!(!parsed.truncated);
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse(b"not json".as_slice()).is_err());
    }

    #[test]
    fn rejects_unresolvable_references() {
        let mut graph =
            container(json!([{"targetId": 99, "mnemonic": "Javac", "outputIds": [10]}]));
        assert!(parse_value(&graph).is_err());
        graph["actions"][0]["targetId"] = json!(1);
        graph["actions"][0]["outputIds"] = json!([99]);
        assert!(parse_value(&graph).is_err());
        graph["actions"][0]["outputIds"] = json!([10]);
        graph["artifacts"][0]["pathFragmentId"] = json!(999);
        assert!(parse_value(&graph).is_err());
    }

    #[test]
    fn rejects_fragment_cycles() {
        let mut graph = container(json!([{"targetId": 1, "mnemonic": "Javac", "outputIds": [10]}]));
        graph["pathFragments"][0]["parentId"] = json!(105);
        assert!(parse_value(&graph).is_err());
    }
}
