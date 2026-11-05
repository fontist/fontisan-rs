//! Convert/extract parity (card 1h-writer) against the Ruby convert goldens.
//!
//! Goldens: `FONTISAN_CONVERT_GOLDENS`, else `<corpus>/goldens/convert`. Missing
//! goldens or corpus skip (and say so). Cases outside this card (outline
//! conversion, WOFF2/collection targets, variable instancing) come back as
//! `fontisan::Unsupported` and are tallied, never counted as passes.
mod common;

use common::{corpus, font_path, hex};
use fontisan::convert::{self, Target, UNSUPPORTED_CLASS};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Ruby's WOFF2 glyf reconstruction bug (fixed in card 0c f2cd08d): Ruby errors,
/// Rust succeeds. Pending until 0c regenerates the goldens.
const PENDING_ON_0C: [(&str, &str); 0] = [];

/// Cases where only `head` may differ from the golden. Empty: the shared
/// Ruby WOFF2 rebuild (sfnt::woff2_view) now gives Ruby's checkSumAdjustment.
const KNOWN_HEAD_DIVERGENCE: [(&str, &str); 0] = [];

/// Goldens that predate the Ruby reference they should describe, replaced by
/// output of that reference (measured 2026-11-05, see HANDOFF-1h-writer.md):
/// * exact ttf/otf: generated before 0b's head-checksum fix (the directory
///   checksum of `head` was taken over the stored checkSumAdjustment); the
///   hashes below come from the phase-0 `lib` (0b included).
/// * ttf-woff2 -> woff: generated before 0c's flag-repeat fix (f2cd08d); the
///   glyf/head hashes come from 0c `lib` at f2cd08d.
///
/// Delete once card 0c regenerates the goldens.
const STALE_EXACT: [(&str, &str, &str); 0] = [];
const STALE_TABLES: [(&str, &str, &str, &str); 0] = [];

/// Sources with CFF outlines whose `format:ttf` golden is an outline
/// conversion (card 1h-o2t); none may fall back to `fontisan::Unsupported`.
const O2T_CASES: [&str; 5] = [
    "fonttools-test-otf",
    "noto-sans-jp-subset-regular-otf",
    "source-sans-3-regular-otf",
    "source-sans-3-regular-otf-woff",
    "source-sans-3-regular-otf-woff2",
];

fn goldens_dir(corpus: &std::path::Path) -> Option<PathBuf> {
    let d = std::env::var_os("FONTISAN_CONVERT_GOLDENS")
        .map(PathBuf::from)
        .unwrap_or_else(|| corpus.join("goldens/convert"));
    if d.is_dir() {
        Some(d)
    } else {
        eprintln!("SKIPPED: no convert goldens at {}", d.display());
        None
    }
}

/// The goldens store error messages with every number run replaced by `N`
/// (Ruby `gsub(/\b\d+\b/, "N")`).
fn normalize_digits(m: &str) -> String {
    let c: Vec<char> = m.chars().collect();
    let word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i].is_ascii_digit() && (i == 0 || !word(c[i - 1])) {
            let mut j = i;
            while j < c.len() && c[j].is_ascii_digit() {
                j += 1;
            }
            if j == c.len() || !word(c[j]) {
                out.push('N');
                i = j;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// Per-table sha256 of an sfnt, or of a WOFF's decompressed tables. WOFF is
/// read directly: Ruby's WoffWriter keeps the source table order, which the
/// strict `decode_woff` (sorted directory required) rejects.
fn decoded_tables(out: &[u8]) -> BTreeMap<String, String> {
    let be32 = |b: &[u8], at: usize| u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as usize;
    if out.starts_with(b"wOFF") {
        let n = u16::from_be_bytes([out[12], out[13]]) as usize;
        return (0..n)
            .map(|i| {
                let e = 44 + 20 * i;
                let (off, comp, orig) = (be32(out, e + 4), be32(out, e + 8), be32(out, e + 12));
                let raw = &out[off..off + comp];
                let data = if comp < orig {
                    miniz_oxide::inflate::decompress_to_vec_zlib(raw).unwrap()
                } else {
                    raw.to_vec()
                };
                (
                    String::from_utf8_lossy(&out[e..e + 4]).into_owned(),
                    sha(&data),
                )
            })
            .collect();
    }
    let n = u16::from_be_bytes([out[4], out[5]]) as usize;
    (0..n)
        .map(|i| {
            let e = 12 + 16 * i;
            let (off, len) = (be32(out, e + 8), be32(out, e + 12));
            (
                String::from_utf8_lossy(&out[e..e + 4]).into_owned(),
                sha(&out[off..off + len]),
            )
        })
        .collect()
}

#[derive(Debug)]
enum Got {
    Ok(Vec<u8>, Option<String>),
    Err(String, String),
}

fn via_rust(input: &[u8], op: &str, to: &str, index: usize) -> Got {
    if op == "extract" {
        return match convert::extract(input, index) {
            Ok(x) => Got::Ok(x.data, Some(x.basename)),
            Err(e) => Got::Err(e.class.into(), e.message),
        };
    }
    let target = match to {
        "ttf" => Target::Ttf,
        "otf" => Target::Otf,
        _ => Target::Woff,
    };
    match convert::convert(input, target, 1_735_689_600) {
        Ok(b) => Got::Ok(b, None),
        Err(e) => Got::Err(e.class.into(), e.message),
    }
}

#[cfg(feature = "ffi")]
fn via_abi(input: &[u8], op: &str, to: &str, index: usize) -> Got {
    use fontisan::ffi::*;
    let mut buf = vec![0u8; 16];
    let (code, opcode) = if op == "extract" {
        (index as u32, OP_EXTRACT)
    } else {
        (
            match to {
                "ttf" => 0,
                "otf" => 1,
                _ => 2,
            },
            OP_CONVERT,
        )
    };
    buf[0..4].copy_from_slice(&code.to_be_bytes());
    buf[8..16].copy_from_slice(&1_735_689_600i64.to_be_bytes());
    buf.extend_from_slice(input);
    let req = FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op: opcode,
        flags: 0,
        input: buf.as_ptr(),
        input_len: buf.len(),
    };
    let mut out = std::ptr::null_mut();
    let status = unsafe { fontisan_call(&req, &mut out) };
    let r = unsafe { &*out };
    let got = if status == 0 {
        let p = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) };
        if op == "extract" {
            let n = u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize;
            Got::Ok(
                p[4 + n..].to_vec(),
                Some(String::from_utf8(p[4..4 + n].to_vec()).unwrap()),
            )
        } else {
            Got::Ok(p.to_vec(), None)
        }
    } else {
        let m = unsafe { std::slice::from_raw_parts(r.error.message, r.error.message_len) };
        let m = String::from_utf8(m.to_vec()).unwrap();
        let (c, msg) = m.split_once(": ").unwrap();
        Got::Err(c.into(), msg.into())
    };
    unsafe { fontisan_result_free(out) };
    got
}

fn check(id: &str, key: &str, case: &Value, got: &Got) -> Result<(), String> {
    match (case["outcome"].as_str().unwrap(), got) {
        ("ok", Got::Ok(bytes, basename)) => {
            if let Some(b) = basename {
                let want = case["output"]["basename"].as_str().unwrap();
                if b != want {
                    return Err(format!("basename {b} != {want}"));
                }
            }
            if case["class"] == "exact" {
                let mut want = case["output"]["sha256"].as_str().unwrap();
                if let Some(s) = STALE_EXACT.iter().find(|s| (s.0, s.1) == (id, key)) {
                    want = s.2;
                }
                if sha(bytes) != want {
                    return Err(format!("sha256 {} != {want}", sha(bytes)));
                }
            } else {
                let mut want: BTreeMap<String, String> =
                    serde_json::from_value(case["decoded_tables"][0].clone()).unwrap();
                for s in STALE_TABLES.iter().filter(|s| (s.0, s.1) == (id, key)) {
                    want.insert(s.2.to_string(), s.3.to_string());
                }
                if decoded_tables(bytes) != want {
                    let diff: Vec<_> = decoded_tables(bytes)
                        .iter()
                        .filter(|(k, v)| want.get(*k) != Some(*v))
                        .map(|(k, _)| k.clone())
                        .collect();
                    return Err(format!("decoded tables differ: {diff:?}"));
                }
            }
            Ok(())
        }
        ("error", Got::Err(class, msg)) => {
            let want = &case["error"];
            if class == want["class"].as_str().unwrap()
                && normalize_digits(msg) == want["message"].as_str().unwrap()
            {
                Ok(())
            } else {
                Err(format!("error {class}: {msg}"))
            }
        }
        (want, got) => Err(format!("golden {want}, got {got:?}")),
    }
}

#[test]
fn convert_and_extract_match_goldens() {
    let Some(corpus) = corpus() else { return };
    let Some(dir) = goldens_dir(&corpus) else {
        return;
    };
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(corpus.join("manifest.json")).unwrap()).unwrap();
    let file_of = |id: &str| -> String {
        manifest["fonts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == id)
            .unwrap()["file"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (mut matched, mut unsupported, mut pending, mut failures) =
        (0, Vec::new(), Vec::new(), Vec::new());
    let mut diverged: Vec<String> = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    for p in paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let g: Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        let id = g["font"].as_str().unwrap();
        let input = std::fs::read(font_path(&corpus, &file_of(id))).unwrap();
        assert_eq!(
            sha(&input),
            g["source_sha256"].as_str().unwrap(),
            "{id} source"
        );
        for (key, case) in g["cases"].as_object().unwrap() {
            let (op, arg) = key.split_once(':').unwrap();
            let in_scope = match op {
                "extract" => true,
                "format" => matches!(arg, "ttf" | "otf" | "woff"),
                _ => false,
            };
            if !in_scope {
                continue;
            }
            let index = if op == "extract" {
                arg.parse().unwrap()
            } else {
                0
            };
            let got = via_rust(&input, op, arg, index);
            if let Got::Err(c, m) = &got {
                if c == UNSUPPORTED_CLASS {
                    unsupported.push(format!("{id} {key}: {m}"));
                    continue;
                }
            }
            if PENDING_ON_0C.contains(&(id, key.as_str())) {
                assert_eq!(case["outcome"], "error");
                assert!(
                    matches!(got, Got::Ok(..)),
                    "{id} {key} pending case must succeed in Rust"
                );
                pending.push(format!("{id} {key}"));
                continue;
            }
            if KNOWN_HEAD_DIVERGENCE.contains(&(id, key.as_str())) {
                let want: BTreeMap<String, String> =
                    serde_json::from_value(case["decoded_tables"][0].clone()).unwrap();
                let Got::Ok(bytes, _) = &got else {
                    panic!("{id} {key}")
                };
                let mut have = decoded_tables(bytes);
                assert_eq!(have.remove("head").is_some(), want.contains_key("head"));
                let mut want = want;
                want.remove("head");
                assert_eq!(have, want, "{id} {key}: only head may differ");
                diverged.push(format!("{id} {key}"));
                continue;
            }
            let mut res = check(id, key, case, &got);
            #[cfg(feature = "ffi")]
            if res.is_ok() {
                res = check(id, key, case, &via_abi(&input, op, arg, index));
            }
            match res {
                Ok(()) => matched += 1,
                Err(e) => failures.push(format!("{id} {key}: {e}")),
            }
        }
    }
    eprintln!("matched {matched}; pending-on-0c {pending:?}; known head divergence {diverged:?}");
    eprintln!("out of card scope ({}):", unsupported.len());
    for u in &unsupported {
        eprintln!("  {u}");
    }
    for id in O2T_CASES {
        assert!(
            !unsupported
                .iter()
                .any(|u| u.starts_with(&format!("{id} format:ttf:"))),
            "{id} format:ttf must be converted by 1h-o2t, not reported unsupported"
        );
    }
    assert!(
        failures.is_empty(),
        "parity failures:\n{}",
        failures.join("\n")
    );
    assert!(
        matched >= 73,
        "only {matched} cases matched; goldens missing?"
    );
}
