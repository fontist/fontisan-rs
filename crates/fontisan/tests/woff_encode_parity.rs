//! The engine's WOFF encode is NOT byte-identical to the Ruby tier's: its
//! pure-Rust DEFLATE (zlib-rs) produces different streams than the libz the
//! Ruby tier links, and the no-C rule bans linking libz. The gates therefore
//! keep `convert to: :woff` with Ruby; this test pins the engine's own
//! deterministic bytes so any encoder change is visible, and documents the
//! divergence the day a byte-exact pure-Rust deflate exists.

mod common;

use common::corpus;
use fontisan::convert::{SourceType, Target, convert};

#[test]
fn woff_encode_pins_the_engine_bytes() {
    let Some(dir) = corpus() else {
        return;
    };
    let input = std::fs::read(dir.join("fonts").join("TestTTF.ttf")).expect("corpus ttf");
    let out = convert(&input, Target::Woff, SourceType::Ttc, 1_735_689_600).expect("converts");
    assert_eq!(out.len(), 1404);
    assert_eq!(
        sha(&out),
        [
            0x26, 0x06, 0xa1, 0x2b, 0x83, 0xe0, 0x5c, 0xed, 0x42, 0x63, 0xcb, 0x7c, 0xeb, 0x08,
            0x0d, 0x6e, 0x52, 0xca, 0xe2, 0x2c, 0x4b, 0x2f, 0xad, 0x4a, 0x8d, 0xdc, 0x61, 0x3c,
            0xa5, 0xcf, 0x69, 0x00,
        ],
        "engine woff bytes (deterministic; the gate keeps the target with Ruby)"
    );
}

fn sha(b: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).into()
}
