// The unit and fuzz tests of the crate

// The fuzz scenarios are assigned field by field, in the order in which they consume the fuzz input
#![allow(clippy::field_reassign_with_default)]

mod containers;
mod support;
mod types;
mod fuzz;
mod cov_frontend;
mod cov_search;
mod cov_buffers;
mod cov_core;
mod cov_misc;
// The Rust-only extensions
mod ext_hints;
