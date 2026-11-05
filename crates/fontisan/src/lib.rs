//! fontisan: pure-Rust font engine.
//!
//! [`sfnt`] is the font model (card 1c); later cards add outlines, layout,
//! audit and conversion. The crate also carries the C ABI scaffolding every
//! entry point will use, behind the `ffi` feature.
#![cfg_attr(not(feature = "ffi"), forbid(unsafe_code))]
#![cfg_attr(feature = "ffi", deny(unsafe_code))]

pub mod audit;
pub mod convert;
pub mod indexability;
pub mod instance;
pub mod json;
pub mod read;
pub mod scan;
pub mod sfnt;
pub mod subset;

#[cfg(feature = "ffi")]
pub mod ffi;

/// Crate version, for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub mod woff;
