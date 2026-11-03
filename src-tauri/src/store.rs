//! Bazel invocation facts in SQLite, with event details in local files.

pub mod analytics;
mod database;
pub mod events;
#[path = "securefs.rs"]
mod securefs;

pub use database::*;
use database::{Store, fail, id};
