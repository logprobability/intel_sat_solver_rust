// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

//! Intel(R) SAT Solver (IntelSAT / Topor): the Rust fork.
//!
//! The fork keeps the structure, the conventions and the identifiers of the C++ solver: every C++ header and translation unit has a corresponding module,
//! and the classes, the functions and the data members retain their C++ names.

// The fork retains the C++ identifiers and layout conventions
#![allow(non_snake_case, non_upper_case_globals, non_camel_case_types)]
#![allow(clippy::all)]
// The fork ports every member the C++ solver defines, including the ones the C++ solver never calls
#![allow(dead_code)]

pub mod basic_memory_usage;
pub mod color_print;
pub mod std_string;
pub mod time_measure;
pub mod topi;
pub mod topi_asg;
pub mod topi_backtrack;
pub mod topi_bcp;
pub mod topi_bit_compression;
pub mod topi_compression;
pub mod topi_conflict_analysis;
pub mod topi_debug_printing;
pub mod topi_decision;
pub mod topi_inprocess;
pub mod topi_main;
pub mod topi_params;
pub mod topi_restart;
pub mod topi_statistics;
pub mod topi_types;
pub mod topi_var_scores;
pub mod topi_wl;
pub mod topor;
pub mod topor_bit_array;
pub mod topor_bit_array_buffer;
pub mod topor_dyn_array;
pub mod topor_external_types;
pub mod topor_ipasir;
pub mod topor_table_bit_info;
pub mod topor_vector;
pub mod topor_win_average;

pub use topor::CTopor;
pub use topor_external_types::*;

// The fuzzing harness: used by the in-crate fuzz tests and by the cargo-fuzz targets (which are built with --cfg fuzzing)
#[cfg(any(test, fuzzing))]
#[path = "tests/fuzzing.rs"]
pub mod fuzzing;

#[cfg(test)]
mod tests;
