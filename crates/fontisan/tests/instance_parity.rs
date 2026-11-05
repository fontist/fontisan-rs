//! Static instancing parity (card 1i-instance) against the Ruby convert goldens:
//! every `instance:*` case, and every variable-font `format:*` case, directly
//! and through the C ABI. Missing goldens or corpus skip (and say so).
//!
//! A WOFF2 output is decoded the way the goldens were (pinned `woff2_decompress`: every
//! glyph padded to 4 bytes, `decode_woff2`), not with Ruby's `Woff2Font` layout
//! (`woff2_to_sfnt`, 1-byte alignment for a long `loca`), which hashes glyf/loca/head
//! differently for the same file. Anything still `fontisan::Unsupported` is tallied as
//! pending, never as a pass.
mod common;

use common::{corpus, font_path, hex};
use fontisan::convert::{self, Target, UNSUPPORTED_CLASS};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn decoded_tables(out: &[u8]) -> BTreeMap<String, String> {
    let sfnt = if out.starts_with(b"wOFF") {
        fontisan::woff::decode_woff(out).unwrap()
    } else if out.starts_with(b"wOF2") {
        fontisan::woff::decode_woff2(out).unwrap()
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

enum Got {
    Ok(Vec<u8>),
    Err(String, String),
}

fn coords(params: &Value) -> Vec<([u8; 4], i32)> {
    params["coords_raw"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            let t = k.as_bytes();
            ([t[0], t[1], t[2], t[3]], v.as_i64().unwrap() as i32)
        })
        .collect()
}

fn instance_direct(input: &[u8], c: &[([u8; 4], i32)]) -> Got {
    let c: Vec<_> = c
        .iter()
        .map(|&(t, r)| (t, f64::from(r) / 65536.0))
        .collect();
    match fontisan::instance::instance(input, &c) {
        Ok(b) => Got::Ok(b),
        Err(e) => Got::Err(e.class.into(), e.message),
    }
}

fn convert_direct(input: &[u8], to: &str) -> Got {
    let t = match to {
        "ttf" => Target::Ttf,
        "otf" => Target::Otf,
        "woff" => Target::Woff,
        _ => Target::Woff2,
    };
    match convert::convert(input, t, 1_735_689_600) {
        Ok(b) => Got::Ok(b),
        Err(e) => Got::Err(e.class.into(), e.message),
    }
}

#[cfg(feature = "ffi")]
fn abi(op: u32, input: Vec<u8>) -> Got {
    use fontisan::ffi::*;
    let req = FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op,
        flags: 0,
        input: input.as_ptr(),
        input_len: input.len(),
    };
    let mut out = std::ptr::null_mut();
    let status = unsafe { fontisan_call(&req, &mut out) };
    let r = unsafe { &*out };
    let got = if status == 0 {
        Got::Ok(unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) }.to_vec())
    } else {
        let m = unsafe { std::slice::from_raw_parts(r.error.message, r.error.message_len) };
        let m = String::from_utf8(m.to_vec()).unwrap();
        let (c, msg) = m.split_once(": ").unwrap();
        Got::Err(c.into(), msg.into())
    };
    unsafe { fontisan_result_free(out) };
    got
}

#[cfg(feature = "ffi")]
fn instance_abi(input: &[u8], c: &[([u8; 4], i32)]) -> Got {
    let mut buf = (c.len() as u32).to_be_bytes().to_vec();
    for (t, r) in c {
        buf.extend_from_slice(t);
        buf.extend_from_slice(&r.to_be_bytes());
    }
    buf.extend_from_slice(input);
    abi(fontisan::ffi::OP_INSTANCE, buf)
}

#[cfg(feature = "ffi")]
fn convert_abi(input: &[u8], to: &str) -> Got {
    let code: u32 = match to {
        "ttf" => 0,
        "otf" => 1,
        "woff" => 2,
        _ => 3,
    };
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&code.to_be_bytes());
    buf[8..16].copy_from_slice(&1_735_689_600i64.to_be_bytes());
    buf.extend_from_slice(input);
    abi(fontisan::ffi::OP_CONVERT, buf)
}

fn check(case: &Value, got: &Got) -> Result<(), String> {
    match (case["outcome"].as_str().unwrap(), got) {
        ("ok", Got::Ok(bytes)) => {
            if case["class"] == "exact" {
                let want = case["output"]["sha256"].as_str().unwrap();
                if sha(bytes) != want {
                    return Err(format!("sha256 {} != {want}", sha(bytes)));
                }
            } else {
                let want: BTreeMap<String, String> =
                    serde_json::from_value(case["decoded_tables"][0].clone()).unwrap();
                let got = decoded_tables(bytes);
                if got != want {
                    let diff: Vec<_> = want
                        .keys()
                        .chain(got.keys())
                        .filter(|k| want.get(*k) != got.get(*k))
                        .collect();
                    return Err(format!("decoded tables differ: {diff:?}"));
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
        (want, Got::Ok(_)) => Err(format!("golden {want}, got ok")),
        (want, Got::Err(c, m)) => Err(format!("golden {want}, got {c}: {m}")),
    }
}

#[test]
fn instance_and_variable_convert_match_goldens() {
    let Some(corpus) = corpus() else { return };
    let dir = std::env::var_os("FONTISAN_CONVERT_GOLDENS")
        .map(PathBuf::from)
        .unwrap_or_else(|| corpus.join("goldens/convert"));
    if !dir.is_dir() {
        eprintln!("SKIPPED: no convert goldens at {}", dir.display());
        return;
    }
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
    let (mut instances, mut converts) = (0, 0);
    let (mut pending, mut failures) = (Vec::new(), Vec::new());
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for p in &paths {
        let g: Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        let id = g["font"].as_str().unwrap();
        let is_vf = id.contains("-vf-");
        let mut input: Option<Vec<u8>> = None;
        for (key, case) in g["cases"].as_object().unwrap() {
            let (op, arg) = key.split_once(':').unwrap();
            if !(op == "instance" || (op == "format" && is_vf)) {
                continue;
            }
            let input = input
                .get_or_insert_with(|| std::fs::read(font_path(&corpus, &file_of(id))).unwrap());
            let (direct, via_abi): (Got, Box<dyn Fn() -> Got>) = if op == "instance" {
                let c = coords(&case["params"]);
                let direct = instance_direct(input, &c);
                let inp = input.clone();
                (direct, Box::new(move || instance_abi_or_skip(&inp, &c)))
            } else {
                let direct = convert_direct(input, arg);
                let (inp, to) = (input.clone(), arg.to_string());
                (direct, Box::new(move || convert_abi_or_skip(&inp, &to)))
            };
            if let Got::Err(c, m) = &direct {
                if c == UNSUPPORTED_CLASS {
                    let owner = if arg == "woff2" {
                        "1h-woff2enc"
                    } else {
                        "1h-t2o"
                    };
                    pending.push(format!("{id} {key}: pending-on-{owner} ({m})"));
                    continue;
                }
            }
            let mut res = check(case, &direct);
            if res.is_ok() {
                if let Some(abi_got) = Some(via_abi()) {
                    if !matches!(&abi_got, Got::Err(c, _) if c == "skipped") {
                        res = check(case, &abi_got).map_err(|e| format!("abi: {e}"));
                    }
                }
            }
            match res {
                Ok(()) if op == "instance" => instances += 1,
                Ok(()) => converts += 1,
                Err(e) => failures.push(format!("{id} {key}: {e}")),
            }
        }
    }
    eprintln!("matched {instances} instance cases, {converts} variable convert cases");
    eprintln!("pending ({}):", pending.len());
    for p in &pending {
        eprintln!("  {p}");
    }
    assert!(
        failures.is_empty(),
        "parity failures:\n{}",
        failures.join("\n")
    );
    assert!(
        instances >= 180,
        "only {instances} instance cases matched; goldens missing?"
    );
}

#[cfg(feature = "ffi")]
fn instance_abi_or_skip(i: &[u8], c: &[([u8; 4], i32)]) -> Got {
    instance_abi(i, c)
}
#[cfg(not(feature = "ffi"))]
fn instance_abi_or_skip(_: &[u8], _: &[([u8; 4], i32)]) -> Got {
    Got::Err("skipped".into(), String::new())
}
#[cfg(feature = "ffi")]
fn convert_abi_or_skip(i: &[u8], to: &str) -> Got {
    convert_abi(i, to)
}
#[cfg(not(feature = "ffi"))]
fn convert_abi_or_skip(_: &[u8], _: &str) -> Got {
    Got::Err("skipped".into(), String::new())
}
