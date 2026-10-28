fn main() {
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path().expect("bundled protoc"));
    config.type_attribute(
        ".blaze.invocation_policy",
        "#[derive(serde::Serialize, serde::Deserialize)] #[serde(rename_all = \"camelCase\")]",
    );
    config.type_attribute(
        ".blaze.strategy_policy",
        "#[derive(serde::Serialize, serde::Deserialize)] #[serde(rename_all = \"camelCase\")]",
    );
    config.type_attribute(
        ".build_event_stream.OptionsParsed",
        "#[derive(serde::Serialize, serde::Deserialize)] #[serde(default, rename_all = \"camelCase\")]",
    );
    let bazel_proto_root = "proto/bazel";
    let bazel_bep = format!(
        "{bazel_proto_root}/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto"
    );
    let includes = vec![
        "proto".to_owned(),
        bazel_proto_root.to_owned(),
        protoc_bin_vendored::include_path()
            .expect("bundled protobuf includes")
            .to_string_lossy()
            .into_owned(),
    ];
    config.include_file("bazel_protos.rs");
    tonic_build::configure()
        .compile_protos_with_config(config, &["proto/bes.proto", &bazel_bep], &includes)
        .expect("compile BES protocol");
    tauri_build::build()
}
