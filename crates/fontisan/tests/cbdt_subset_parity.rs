//! CBDT/CBLC subset parity: Rust's `subset::subset` (web profile) must equal
//! Ruby's `Reference.subset` byte for byte on fonts with colour bitmaps.
//!
//! Fixtures live in `tests/cbdt_subset/` (regenerate with `gen_fixtures.rb`):
//! the input font, Ruby's expected output, and `manifest.json` with the kept
//! glyph ids and both sha256 digests. No corpus font contains CBDT, so these
//! synthetic fonts are the only evidence.
use fontisan::subset;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cbdt_subset")
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn cases() -> Vec<Value> {
    let m = std::fs::read_to_string(dir().join("manifest.json")).unwrap();
    serde_json::from_str::<Vec<Value>>(&m).unwrap()
}

fn table<'a>(font: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let n = u16::from_be_bytes([font[4], font[5]]) as usize;
    (0..n).find_map(|i| {
        let r = &font[12 + 16 * i..28 + 16 * i];
        (&r[..4] == tag).then(|| {
            let off = u32::from_be_bytes(r[8..12].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(r[12..16].try_into().unwrap()) as usize;
            &font[off..off + len]
        })
    })
}

#[test]
fn cbdt_subset_matches_ruby_byte_for_byte() {
    let all = cases();
    assert!(all.len() >= 5);
    for c in all {
        let name = c["name"].as_str().unwrap();
        let input = std::fs::read(dir().join(format!("{name}.ttf"))).unwrap();
        let expected = std::fs::read(dir().join(format!("{name}.expected.ttf"))).unwrap();
        assert_eq!(sha(&input), c["input_sha256"], "{name}: input fixture");
        assert_eq!(
            sha(&expected),
            c["expected_sha256"],
            "{name}: expected fixture"
        );
        let gids: Vec<u32> = c["gids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        let got = subset::subset(&input, &gids, "web")
            .unwrap_or_else(|e| panic!("{name}: rust subset failed: {e}"));
        // table-level first, so a mismatch names the table
        for tag in [b"CBDT", b"CBLC"] {
            assert_eq!(
                table(&got, tag),
                table(&expected, tag),
                "{name}: {} differs from ruby",
                String::from_utf8_lossy(tag)
            );
        }
        assert_eq!(
            sha(&got),
            c["expected_sha256"],
            "{name}: whole font differs from ruby"
        );
        assert_eq!(got, expected, "{name}");
    }
}

#[test]
fn strike_without_a_surviving_glyph_is_dropped() {
    // multi_strike keeps gids 2,3,5,7,8,9: the 48 ppem strike (gids 11-12) goes.
    let num_sizes = |n: &str| {
        let f = std::fs::read(dir().join(format!("{n}.expected.ttf"))).unwrap();
        let cblc = table(&f, b"CBLC").unwrap().to_vec();
        u32::from_be_bytes(cblc[4..8].try_into().unwrap())
    };
    assert_eq!(num_sizes("multi_strike"), 2);
    assert_eq!(num_sizes("multi_strike_all"), 3);
}
