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
const PENDING_ON_0C: [(&str, &str); 2] = [
    ("source-sans-3-regular-ttf-woff2", "format:ttf"),
    // Golden predates 0c's WOFF2 flag fix (f2cd08d); glyf already matches it.
    ("source-sans-3-regular-ttf-woff2", "format:woff"),
];

/// Goldens that predate the Ruby reference they should describe, replaced by
/// output of that reference (measured 2026-11-05, see HANDOFF-1h-writer.md):
/// * exact ttf/otf: generated before 0b's head-checksum fix (the directory
///   checksum of `head` was taken over the stored checkSumAdjustment); the
///   hashes below come from the phase-0 `lib` (0b included).
/// * ttf-woff2 -> woff: generated before 0c's flag-repeat fix (f2cd08d); the
///   glyf/head hashes come from 0c `lib` at f2cd08d.
///
/// Delete once card 0c regenerates the goldens.
const STALE_EXACT: [(&str, &str, &str); 5] = [
    (
        "fonttools-test-woff2",
        "format:ttf",
        "d8e57cbb280228b8de65c6b657da32165085382964445f115ee44c25ad684f73",
    ),
    (
        "fonttools-test-woff",
        "format:otf",
        "44f44826395864619fdc16b0cad52ae441c7d921173e341b8e510d58ed57a285",
    ),
    (
        "quirk-sfnt-version-otto-on-glyf",
        "format:ttf",
        "db9872dcbf836a7e94ed001a0c935e3f0ba4b37977ba1f9f3bead5b21ef052a7",
    ),
    (
        "source-sans-3-regular-otf-woff",
        "format:otf",
        "36cf68503a2640878fb0d03db9edd2b42115191bd6216e1a80b8c9290bfafc6d",
    ),
    (
        "source-sans-3-regular-ttf-woff",
        "format:ttf",
        "a3965d4a715b9bd6c202ec607b62831ba724e68168da904d174235552ca053fe",
    ),
];
const STALE_TABLES: [(&str, &str, &str, &str); 2] = [
    (
        "source-sans-3-regular-ttf-woff2",
        "format:woff",
        "glyf",
        "08ad5d17e51b58c2d205730892a80c43c097b543d6ea1b2576e4db3a6366a072",
    ),
    (
        "source-sans-3-regular-ttf-woff2",
        "format:woff",
        "head",
        "d5b5d6f84513bf55a04271e366258b5a900389268c53c1c67c1ab90720be0fb6",
    ),
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

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// Per-table sha256 of an sfnt, decoding WOFF first.
fn decoded_tables(out: &[u8]) -> BTreeMap<String, String> {
    let sfnt = if out.starts_with(b"wOFF") {
        fontisan::woff::decode_woff(out).unwrap()
    } else {
        out.to_vec()
    };
    let n = u16::from_be_bytes([sfnt[4], sfnt[5]]) as usize;
    (0..n)
        .map(|i| {
            let e = &sfnt[12 + 16 * i..28 + 16 * i];
            let off = u32::from_be_bytes(e[8..12].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(e[12..16].try_into().unwrap()) as usize;
            (
                String::from_utf8_lossy(&e[0..4]).into_owned(),
                sha(&sfnt[off..off + len]),
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
                    return Err("decoded tables differ".into());
                }
            }
            Ok(())
        }
        ("error", Got::Err(class, msg)) => {
            let want = &case["error"];
            if class == want["class"].as_str().unwrap() && msg == want["message"].as_str().unwrap()
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
    eprintln!("matched {matched}; pending-on-0c {pending:?}");
    eprintln!("out of card scope ({}):", unsupported.len());
    for u in &unsupported {
        eprintln!("  {u}");
    }
    assert!(
        failures.is_empty(),
        "parity failures:\n{}",
        failures.join("\n")
    );
    assert!(
        matched >= 60,
        "only {matched} cases matched; goldens missing?"
    );
}
