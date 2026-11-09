use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;

// Bounds displayed actions; aquery still reads the full graph and retained action details.
const MAX_ACTIONS: usize = 100;

/// A key/value pair carried by an action, such as an environment variable.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionKeyValue {
    pub key: String,
    pub value: String,
}

/// Public metadata of the configuration an action runs under.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionConfiguration {
    pub mnemonic: String,
    pub platform_name: String,
    pub checksum: String,
    pub is_tool: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetAction {
    pub output: String,
    pub mnemonic: String,
    pub owner: String,
    pub arguments: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub environment_variables: Vec<ActionKeyValue>,
    pub configuration: Option<ActionConfiguration>,
    pub execution_platform: String,
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
    #[serde(default)]
    dep_set_of_files: Vec<DepSetOfFilesProto>,
    #[serde(default)]
    configuration: Vec<ConfigurationProto>,
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
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    input_dep_set_ids: Vec<u32>,
    #[serde(default)]
    environment_variables: Vec<KeyValuePairProto>,
    #[serde(default)]
    configuration_id: u32,
    #[serde(default)]
    execution_platform: String,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DepSetOfFilesProto {
    id: u32,
    #[serde(default)]
    transitive_dep_set_ids: Vec<u32>,
    #[serde(default)]
    direct_artifact_ids: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigurationProto {
    id: u32,
    #[serde(default)]
    mnemonic: String,
    #[serde(default)]
    platform_name: String,
    #[serde(default)]
    checksum: String,
    #[serde(default)]
    is_tool: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyValuePairProto {
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: String,
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
    let dep_sets: HashMap<u32, DepSetOfFilesProto> = graph
        .dep_set_of_files
        .into_iter()
        .map(|set| (set.id, set))
        .collect();
    let configurations: HashMap<u32, ConfigurationProto> = graph
        .configuration
        .into_iter()
        .map(|configuration| (configuration.id, configuration))
        .collect();
    let truncated = graph.actions.len() > MAX_ACTIONS;
    let mut actions = Vec::with_capacity(graph.actions.len().min(MAX_ACTIONS));
    for action in graph.actions.iter().take(MAX_ACTIONS) {
        actions.push(summarize(
            action,
            &targets,
            &dep_sets,
            &configurations,
            &artifacts,
            &fragments,
        )?);
    }
    Ok(TargetActions { actions, truncated })
}

fn summarize(
    action: &ActionProto,
    targets: &HashMap<u32, String>,
    dep_sets: &HashMap<u32, DepSetOfFilesProto>,
    configurations: &HashMap<u32, ConfigurationProto>,
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
    let outputs = action
        .output_ids
        .iter()
        .map(|id| artifact_path(*id, artifacts, fragments))
        .collect::<Result<Vec<_>, _>>()?;
    let inputs = input_paths(action, dep_sets, artifacts, fragments)?;
    let configuration = if action.configuration_id == 0 {
        None
    } else {
        let configuration = configurations
            .get(&action.configuration_id)
            .ok_or_else(|| "aquery action references an unknown configuration".to_string())?;
        Some(ActionConfiguration {
            mnemonic: configuration.mnemonic.clone(),
            platform_name: configuration.platform_name.clone(),
            checksum: configuration.checksum.clone(),
            is_tool: configuration.is_tool,
        })
    };
    Ok(TargetAction {
        output,
        mnemonic: action.mnemonic.clone(),
        owner: owner.clone(),
        arguments: action.arguments.clone(),
        inputs,
        outputs,
        environment_variables: action
            .environment_variables
            .iter()
            .map(|pair| ActionKeyValue {
                key: pair.key.clone(),
                value: pair.value.clone(),
            })
            .collect(),
        configuration,
        execution_platform: action.execution_platform.clone(),
    })
}

/// Expands the action's input dep sets into artifact exec paths. Dep sets and
/// artifacts are deduplicated at first traversal encounter; a dep set cycle is
/// an error rather than a hang.
fn input_paths(
    action: &ActionProto,
    dep_sets: &HashMap<u32, DepSetOfFilesProto>,
    artifacts: &HashMap<u32, u32>,
    fragments: &HashMap<u32, PathFragmentProto>,
) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    let mut seen_artifacts = HashSet::new();
    let mut active = HashSet::new();
    let mut done = HashSet::new();
    let mut stack: Vec<_> = action
        .input_dep_set_ids
        .iter()
        .rev()
        .map(|&id| (id, false))
        .collect();
    while let Some((id, exiting)) = stack.pop() {
        if exiting {
            active.remove(&id);
            done.insert(id);
            continue;
        }
        if done.contains(&id) {
            continue;
        }
        if !active.insert(id) {
            return Err("aquery input dep sets contain a cycle".to_string());
        }
        let set = dep_sets
            .get(&id)
            .ok_or_else(|| "aquery action input references an unknown dep set".to_string())?;
        for artifact_id in set
            .direct_artifact_ids
            .iter()
            .copied()
            .filter(|id| seen_artifacts.insert(*id))
        {
            paths.push(artifact_path(artifact_id, artifacts, fragments)?);
        }
        stack.push((id, true));
        stack.extend(
            set.transitive_dep_set_ids
                .iter()
                .rev()
                .map(|&id| (id, false)),
        );
    }
    Ok(paths)
}

fn artifact_path(
    artifact_id: u32,
    artifacts: &HashMap<u32, u32>,
    fragments: &HashMap<u32, PathFragmentProto>,
) -> Result<String, String> {
    let mut fragment_id = *artifacts
        .get(&artifact_id)
        .ok_or_else(|| "aquery artifact is not a known artifact".to_string())?;
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
            "depSetOfFiles": [
                {"id": 20, "directArtifactIds": [10]},
                {"id": 21, "directArtifactIds": [11], "transitiveDepSetIds": [20]},
                {"id": 22, "directArtifactIds": [10, 11], "transitiveDepSetIds": [21]},
            ],
            "configuration": [
                {
                    "id": 30,
                    "mnemonic": "k8-fastbuild",
                    "platformName": "@local_config_platform//:host",
                    "checksum": "abc123",
                    "isTool": false,
                },
                {
                    "id": 31,
                    "mnemonic": "k8-opt-exec",
                    "platformName": "@local_config_platform//:host",
                    "checksum": "def456",
                    "isTool": true,
                },
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
    fn resolves_every_output_id() {
        let parsed = parse_value(&container(json!([
            {"targetId": 1, "mnemonic": "Javac", "outputIds": [10, 11], "primaryOutputId": 10},
        ])))
        .unwrap();
        assert_eq!(
            parsed.actions[0].outputs,
            [
                "bazel-out/k8-fastbuild/bin/app/server_deploy.jar",
                "bazel-out/k8-fastbuild/bin/app/server"
            ]
        );
        assert_eq!(parsed.actions[0].output, parsed.actions[0].outputs[0]);
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
    fn expands_inputs_deduplicating_shared_dep_sets_and_artifacts() {
        let parsed = parse_value(&container(json!([
            {
                "targetId": 1,
                "mnemonic": "Javac",
                "inputDepSetIds": [22, 21, 20],
                "outputIds": [10],
            },
        ])))
        .unwrap();
        // Set 22 lists artifacts 10 and 11 directly, then set 21 contributes
        // nothing new through set 20; roots 21 and 20 are already expanded.
        assert_eq!(
            parsed.actions[0].inputs,
            [
                "bazel-out/k8-fastbuild/bin/app/server_deploy.jar",
                "bazel-out/k8-fastbuild/bin/app/server"
            ]
        );
    }

    #[test]
    fn expands_inputs_in_first_encounter_order() {
        let parsed = parse_value(&container(json!([
            {"targetId": 1, "mnemonic": "Javac", "inputDepSetIds": [21], "outputIds": [10]},
        ])))
        .unwrap();
        // Set 21's own artifact precedes the artifact of its transitive set 20.
        assert_eq!(
            parsed.actions[0].inputs,
            [
                "bazel-out/k8-fastbuild/bin/app/server",
                "bazel-out/k8-fastbuild/bin/app/server_deploy.jar"
            ]
        );
    }

    #[test]
    fn rejects_missing_input_dep_sets_and_artifacts() {
        let mut graph = container(json!([
            {"targetId": 1, "mnemonic": "Javac", "inputDepSetIds": [99], "outputIds": [10]},
        ]));
        assert!(parse_value(&graph).is_err());
        graph["actions"][0]["inputDepSetIds"] = json!([20]);
        graph["depSetOfFiles"][0]["directArtifactIds"] = json!([99]);
        assert!(parse_value(&graph).is_err());
    }

    #[test]
    fn rejects_dep_set_cycles() {
        let mut graph = container(json!([
            {"targetId": 1, "mnemonic": "Javac", "inputDepSetIds": [40], "outputIds": [10]},
        ]));
        graph["depSetOfFiles"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": 40, "transitiveDepSetIds": [41]}));
        graph["depSetOfFiles"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": 41, "transitiveDepSetIds": [40]}));
        assert!(parse_value(&graph).is_err());

        let mut graph = container(json!([
            {"targetId": 1, "mnemonic": "Javac", "inputDepSetIds": [42], "outputIds": [10]},
        ]));
        graph["depSetOfFiles"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": 42, "transitiveDepSetIds": [42]}));
        assert!(parse_value(&graph).is_err());
    }

    #[test]
    fn includes_arguments_environment_and_execution_platform() {
        let parsed = parse_value(&container(json!([
            {
                "targetId": 1,
                "mnemonic": "CppCompile",
                "outputIds": [10],
                "primaryOutputId": 10,
                "arguments": ["gcc", "-c", "app/main.c", "-o", "out.o"],
                "environmentVariables": [{"key": "FOO", "value": "a b"}],
                "executionPlatform": "@local_config_platform//:host",
            },
        ])))
        .unwrap();
        let action = &parsed.actions[0];
        assert_eq!(action.arguments, ["gcc", "-c", "app/main.c", "-o", "out.o"]);
        assert_eq!(action.environment_variables.len(), 1);
        assert_eq!(action.environment_variables[0].key, "FOO");
        assert_eq!(action.environment_variables[0].value, "a b");
        assert_eq!(action.execution_platform, "@local_config_platform//:host");
        assert!(action.configuration.is_none());
    }

    #[test]
    fn defaults_empty_detail_fields() {
        let parsed = parse_value(&container(json!([
            {"targetId": 1, "mnemonic": "Symlink", "outputIds": []},
        ])))
        .unwrap();
        let action = &parsed.actions[0];
        assert!(action.configuration.is_none());
        assert!(action.environment_variables.is_empty());
        assert!(action.arguments.is_empty());
        assert!(action.inputs.is_empty());
        assert!(action.outputs.is_empty());
        assert_eq!(action.execution_platform, "");
    }

    #[test]
    fn resolves_present_and_missing_configurations() {
        let parsed = parse_value(&container(json!([
            {"targetId": 1, "mnemonic": "Javac", "outputIds": [10], "configurationId": 30},
            {"targetId": 1, "mnemonic": "Javac", "outputIds": [10], "configurationId": 31},
            {"targetId": 1, "mnemonic": "Symlink", "outputIds": []},
        ])))
        .unwrap();
        let first = parsed.actions[0].configuration.as_ref().unwrap();
        assert_eq!(first.mnemonic, "k8-fastbuild");
        assert_eq!(first.platform_name, "@local_config_platform//:host");
        assert_eq!(first.checksum, "abc123");
        assert!(!first.is_tool);
        let second = parsed.actions[1].configuration.as_ref().unwrap();
        assert_eq!(second.mnemonic, "k8-opt-exec");
        assert!(second.is_tool);
        assert!(parsed.actions[2].configuration.is_none());
    }

    #[test]
    fn rejects_unknown_configurations() {
        let mut graph = container(json!([
            {"targetId": 1, "mnemonic": "Javac", "outputIds": [10], "configurationId": 99},
        ]));
        assert!(parse_value(&graph).is_err());
        graph["actions"][0]["configurationId"] = json!(0);
        assert!(
            parse_value(&graph).unwrap().actions[0]
                .configuration
                .is_none()
        );
    }

    #[test]
    fn caps_retained_actions_at_one_hundred() {
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
