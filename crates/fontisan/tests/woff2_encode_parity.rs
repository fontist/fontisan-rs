//! WOFF2 encode parity (card 1h-woff2enc): every single-font `format:woff2` convert golden in
//! the `decoded-exact` class must decode to the same table set with byte-equal tables.
//!
//! Goldens: `FONTISAN_CONVERT_GOLDENS`, default `<corpus>/goldens/convert`. Sources are found
//! by the golden's `source_sha256` in `corpus/fonts` and `corpus/derived`. Pinned decoder
//! check: `FONTISAN_WOFF2_DECOMPRESS` (a `woff2_decompress` binary); skipped when unset/absent.
mod common;

use fontisan::convert::woff2::{encode_woff2, encode_woff2_with_quality};
use fontisan::woff::{decode_woff, decode_woff2};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const TIMESTAMP: i64 = 1_735_689_600;

fn goldens_dir() -> PathBuf {
    match std::env::var_os("FONTISAN_CONVERT_GOLDENS") {
        Some(d) => PathBuf::from(d),
        None => common::corpus_dir().join("corpus/goldens/convert"),
    }
}

fn sha(b: &[u8]) -> String {
    common::hex(&Sha256::digest(b))
}

fn tables(sfnt: &[u8]) -> BTreeMap<String, String> {
    let n = usize::from(u16::from_be_bytes([sfnt[4], sfnt[5]]));
    (0..n)
        .map(|i| {
            let e = &sfnt[12 + 16 * i..28 + 16 * i];
            let off = u32::from_be_bytes([e[8], e[9], e[10], e[11]]) as usize;
            let len = u32::from_be_bytes([e[12], e[13], e[14], e[15]]) as usize;
            (
                String::from_utf8_lossy(&e[0..4]).into_owned(),
                sha(&sfnt[off..off + len]),
            )
        })
        .collect()
}

fn has_table(sfnt: &[u8], tag: &str) -> bool {
    tables(sfnt).contains_key(tag)
}

struct Case {
    name: String,
    source: Vec<u8>,
    source_format: String,
    expected: BTreeMap<String, String>,
}

fn cases() -> Option<Vec<Case>> {
    let dir = goldens_dir();
    let corpus = common::corpus()?;
    if !dir.is_dir() {
        if std::env::var_os("FONTISAN_SPIKE_REQUIRE_CORPUS").is_some() {
            panic!("convert goldens missing at {}", dir.display());
        }
        eprintln!("SKIPPED: convert goldens not found at {}", dir.display());
        return None;
    }
    let mut by_sha: BTreeMap<String, PathBuf> = BTreeMap::new();
    for d in ["fonts", "derived"] {
        if let Ok(rd) = std::fs::read_dir(corpus.join(d)) {
            for e in rd.flatten() {
                if e.path().is_file() {
                    by_sha.insert(sha(&std::fs::read(e.path()).unwrap()), e.path());
                }
            }
        }
    }
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let g: Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        let Some(c) = g["cases"].get("format:woff2") else {
            continue;
        };
        if c["outcome"] != "ok" || c["class"] != "decoded-exact" {
            continue;
        }
        let fmt = g["source_format"].as_str().unwrap().to_string();
        if !["ttf", "otf", "woff"].contains(&fmt.as_str()) {
            continue; // collections are the collection encoder's cards
        }
        let src = by_sha
            .get(g["source_sha256"].as_str().unwrap())
            .unwrap_or_else(|| panic!("source for {} not in corpus", f.display()));
        let expected = c["decoded_tables"][0]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        out.push(Case {
            name: g["font"].as_str().unwrap().to_string(),
            source: std::fs::read(src).unwrap(),
            source_format: fmt,
            expected,
        });
    }
    Some(out)
}

fn sfnt_of(c: &Case) -> Vec<u8> {
    if c.source_format == "woff" {
        decode_woff(&c.source).unwrap()
    } else {
        c.source.clone()
    }
}

/// Variable fonts go through instancing in Ruby (card 1i); not this card.
fn pending_1i(sfnt: &[u8]) -> bool {
    has_table(sfnt, "fvar")
}

fn pinned_decoder() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("FONTISAN_WOFF2_DECOMPRESS")?);
    p.is_file().then_some(p)
}

// Test-only: the engine never spawns, but the pinned reference decoder is an external binary.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
fn run_pinned(bin: &Path, woff2: &[u8], name: &str) -> Vec<u8> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("woff2enc");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join(format!("{name}.woff2"));
    std::fs::write(&f, woff2).unwrap();
    let st = std::process::Command::new(bin).arg(&f).status().unwrap();
    assert!(st.success(), "{name}: woff2_decompress failed");
    std::fs::read(f.with_extension("ttf")).unwrap()
}

#[test]
fn single_font_goldens_are_decoded_exact() {
    let Some(cases) = cases() else { return };
    let pinned = pinned_decoder();
    let (mut checked, mut pending) = (0, Vec::new());
    for c in &cases {
        let sfnt = sfnt_of(c);
        if pending_1i(&sfnt) {
            pending.push(c.name.clone());
            continue;
        }
        // Decoded tables do not depend on the Brotli level; the default (11) runs on small
        // fonts; the ABI test mirrors that through flags bits 8..=15.
        let q = if sfnt.len() < 20_000 { 11 } else { 5 };
        let out = encode_woff2_with_quality(&sfnt, Some(TIMESTAMP), q)
            .unwrap_or_else(|e| panic!("{}: {e}", c.name));
        let decoded = decode_woff2(&out).unwrap_or_else(|e| panic!("{}: {e}", c.name));
        assert_eq!(
            tables(&decoded),
            c.expected,
            "{}: decoded tables differ",
            c.name
        );
        if let Some(bin) = &pinned {
            let theirs = run_pinned(bin, &out, &c.name);
            assert_eq!(
                theirs, decoded,
                "{}: pinned decoder differs from ours",
                c.name
            );
        }
        checked += 1;
    }
    eprintln!(
        "woff2 encode parity: {checked} matched, pending 1i: {pending:?}, pinned={}",
        pinned.is_some()
    );
    assert!(checked >= 15, "too few cases checked: {checked}");
}

#[cfg(feature = "ffi")]
#[test]
fn single_font_goldens_through_the_c_abi() {
    use fontisan::ffi::*;
    let Some(cases) = cases() else { return };
    let mut checked = 0;
    for c in &cases {
        let sfnt = sfnt_of(c);
        if pending_1i(&sfnt) {
            continue;
        }
        let mut input = TIMESTAMP.to_le_bytes().to_vec();
        input.extend_from_slice(&sfnt);
        let req = FontisanRequest {
            header: FontisanHeader::current::<FontisanRequest>(),
            op: OP_WOFF2_ENCODE,
            flags: WOFF2_FLAG_TIMESTAMP | if sfnt.len() < 20_000 { 0 } else { 6 << 8 },
            input: input.as_ptr(),
            input_len: input.len(),
        };
        let mut res = std::ptr::null_mut();
        let status = unsafe { fontisan_call(&req, &mut res) };
        assert_eq!(status, 0, "{}", c.name);
        let r = unsafe { &*res };
        let payload = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) }.to_vec();
        unsafe { fontisan_result_free(res) };
        if sfnt.len() < 20_000 {
            assert_eq!(
                payload,
                encode_woff2(&sfnt, Some(TIMESTAMP)).unwrap(),
                "{}",
                c.name
            );
        }
        let decoded = decode_woff2(&payload).unwrap();
        assert_eq!(
            tables(&decoded),
            c.expected,
            "{}: ABI decoded tables differ",
            c.name
        );
        checked += 1;
    }
    assert!(checked >= 15);
}

#[test]
fn rejects_non_fonts_and_missing_tables() {
    assert!(encode_woff2(b"garbage-not-a-font", Some(0)).is_err());
    // sfnt header with zero tables
    let mut empty = vec![0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    empty.extend_from_slice(&[0; 4]);
    let e = encode_woff2(&empty, Some(0)).unwrap_err();
    assert!(e.to_string().contains("missing required table"), "{e}");
}
