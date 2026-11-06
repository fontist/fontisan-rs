#![no_main]
#[path = "../bodies.rs"]
mod bodies;

libfuzzer_sys::fuzz_target!(|data: &[u8]| bodies::woff2(data));
