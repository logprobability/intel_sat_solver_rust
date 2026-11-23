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

// The public API: CTopor (and the types it uses), IPASIR, and the C++ library helpers the command-line tool uses
pub mod color_print;
pub mod std_string;
pub mod topor;
pub mod topor_external_types;
pub mod topor_ipasir;

// The solver internals: their raw views (Span, CCls, CBitArray, ...) are as unchecked as their C++ counterparts, so they are not exposed to safe code outside the crate
pub(crate) mod basic_memory_usage;
pub(crate) mod time_measure;
pub(crate) mod topi;
pub(crate) mod topi_asg;
pub(crate) mod topi_backtrack;
pub(crate) mod topi_bcp;
pub(crate) mod topi_bit_compression;
pub(crate) mod topi_compression;
pub(crate) mod topi_conflict_analysis;
pub(crate) mod topi_debug_printing;
pub(crate) mod topi_decision;
pub(crate) mod topi_inprocess;
pub(crate) mod topi_main;
pub(crate) mod topi_params;
pub(crate) mod topi_restart;
pub(crate) mod topi_statistics;
pub(crate) mod topi_types;
pub(crate) mod topi_var_scores;
pub(crate) mod topi_wl;
pub(crate) mod topor_bit_array;
pub(crate) mod topor_bit_array_buffer;
pub(crate) mod topor_dyn_array;
pub(crate) mod topor_table_bit_info;
pub(crate) mod topor_vector;
pub(crate) mod topor_win_average;

pub use topor::CTopor;
pub use topor_external_types::*;

// The fuzzing harness: used by the in-crate fuzz tests and by the cargo-fuzz targets (which are built with --cfg fuzzing)
#[cfg(any(test, fuzzing))]
#[path = "tests/fuzzing.rs"]
pub mod fuzzing;

#[cfg(test)]
mod tests;
