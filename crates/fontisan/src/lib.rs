//! fontisan: pure-Rust font engine.
//!
//! Parsing lands in later cards (1b onward). This crate currently carries the
//! C ABI scaffolding every entry point will use, behind the `ffi` feature.
#![cfg_attr(not(feature = "ffi"), forbid(unsafe_code))]
#![cfg_attr(feature = "ffi", deny(unsafe_code))]

#[cfg(feature = "ffi")]
pub mod ffi;

/// Crate version, for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
