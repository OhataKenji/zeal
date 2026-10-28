//! SQLite run summaries. Bazel still writes observations into each run directory.

pub mod analytics;
mod database;
pub mod events;
#[path = "securefs.rs"]
mod securefs;

pub use database::*;
use database::{Store, fail};
