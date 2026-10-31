// Keep upstream generated code untouched by local lint policy. `all` does not
// include the pedantic function-length lint enabled by this crate.
#[allow(clippy::all, clippy::too_many_lines, dead_code)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/bazel_protos.rs"));
}

pub mod bes {
    pub use super::generated::google::devtools::build::v1::*;
}
pub mod bep {
    pub use super::generated::build_event_stream::*;
}
