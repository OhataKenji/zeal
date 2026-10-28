#[allow(clippy::all, dead_code)] // Keep upstream generated code untouched by local lint policy.
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/bazel_protos.rs"));
}

pub mod bes {
    pub use super::generated::google::devtools::build::v1::*;
}
pub mod bep {
    pub use super::generated::build_event_stream::*;
}
