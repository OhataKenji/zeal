fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("bundled protoc");
    // SAFETY: the build script is single threaded before spawning protoc.
    unsafe { std::env::set_var("PROTOC", protoc) };
    tonic_build::configure()
        .codec_path("crate::bes::CountingCodec")
        .extern_path(
            ".google.devtools.build.v1.PublishBuildToolEventStreamRequest",
            "crate::bes::WireRequest",
        )
        .compile_protos(&["proto/bes.proto", "proto/bep.proto"], &["proto"])
        .expect("compile BES wire projection");
    println!("cargo:rerun-if-changed=proto");
    tauri_build::build()
}
