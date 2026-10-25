fn main() {
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path().expect("bundled protoc"));
    tonic_build::configure()
        .compile_protos_with_config(config, &["proto/bes.proto", "proto/bep.proto"], &["proto"])
        .expect("compile BES protocol");
    tauri_build::build()
}
