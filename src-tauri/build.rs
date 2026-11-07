fn main() {
    let mut config = tonic_prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path().expect("bundled protoc"));
    let bazel_proto_root = "proto/bazel";
    let bazel_bep = format!(
        "{bazel_proto_root}/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto"
    );
    let bazel_query = format!("{bazel_proto_root}/src/main/protobuf/build.proto");
    let bazel_analysis = format!("{bazel_proto_root}/src/main/protobuf/analysis_v2.proto");
    let protobuf_include = protoc_bin_vendored::include_path()
        .expect("bundled protobuf includes")
        .to_string_lossy()
        .into_owned();
    // proto/bazel must come first so vendored imports like
    // "src/main/protobuf/build.proto" resolve to the same virtual file.
    let includes = [bazel_proto_root, "proto", protobuf_include.as_str()];
    config.include_file("bazel_protos.rs");
    tonic_prost_build::configure()
        .compile_with_config(
            config,
            &["proto/bes.proto", &bazel_bep, &bazel_query, &bazel_analysis],
            &includes,
        )
        .expect("compile Bazel protocols");
    tauri_build::build()
}
