//! Thin cdylib wrapper. A cdylib exports the engine's `#[no_mangle]` symbols
//! only when they are re-exported from this crate root.
pub use engine::ffi::*;
