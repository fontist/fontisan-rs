//! Subset parity (card 1i-subset) against the `subset:*` cases of the Ruby
//! convert goldens (`Reference.subset`, profile "web").
//!
//! Goldens: `FONTISAN_CONVERT_GOLDENS`, else `<corpus>/goldens/convert`. Missing
//! goldens or corpus skip (and say so). The golden holds the facade inputs
//! (text or codepoints), so the unicode to gid step runs here, as the facade
//! does it. A case Rust cannot yet produce comes back as `fontisan::Unsupported`
//! and is tallied, never counted as a pass.
mod common;

use common::{corpus, font_path, hex};
use fontisan::subset::{self, UNSUPPORTED_CLASS};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

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

#[derive(Debug)]
enum Got {
    Ok(Vec<u8>),
    Err(String, String),
}

fn codepoints(params: &Value) -> Vec<u32> {
    let mut cps: Vec<u32> = params["unicodes"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_u64().unwrap() as u32).collect())
        .unwrap_or_default();
    if let Some(t) = params["text"].as_str() {
        cps.extend(t.chars().map(|c| c as u32));
    }
    cps
}

fn via_rust(input: &[u8], gids: &[u32]) -> Got {
    match subset::subset(input, gids, "web") {
        Ok(b) => Got::Ok(b),
        Err(e) => Got::Err(e.class.into(), e.message),
    }
}

#[cfg(feature = "ffi")]
fn via_abi(input: &[u8], gids: &[u32]) -> Got {
    use fontisan::ffi::*;
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&1u32.to_be_bytes());
    buf[4..8].copy_from_slice(&(gids.len() as u32).to_be_bytes());
    for g in gids {
        buf.extend_from_slice(&g.to_be_bytes());
    }
    buf.extend_from_slice(input);
    let req = FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op: OP_SUBSET,
        flags: 0,
        input: buf.as_ptr(),
        input_len: buf.len(),
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

fn check(case: &Value, got: &Got) -> Result<(), String> {
    match (case["outcome"].as_str().unwrap(), got) {
        ("ok", Got::Ok(bytes)) => {
            let want = case["output"]["sha256"].as_str().unwrap();
            let have = hex(&Sha256::digest(bytes));
            if have == want {
                Ok(())
            } else {
                Err(format!("sha256 {have} != {want}"))
            }
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
fn subset_matches_goldens() {
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
    let (mut matched, mut unsupported, mut failures) = (0, Vec::new(), Vec::new());
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
        let name = file_of(id);
        let input = std::fs::read(font_path(&corpus, &name)).unwrap();
        for (key, case) in g["cases"].as_object().unwrap() {
            if !key.starts_with("subset:") {
                continue;
            }
            let cps = codepoints(&case["params"]);
            let run = |f: &dyn Fn(&[u32]) -> Got| match subset::glyph_ids_for(&input, &cps, &name) {
                Ok(gids) => f(&gids),
                Err(e) => Got::Err(e.class.into(), e.message),
            };
            let got = run(&|gids| via_rust(&input, gids));
            if let Got::Err(c, m) = &got {
                if c == UNSUPPORTED_CLASS {
                    unsupported.push(format!("{id} {key}: {m}"));
                    continue;
                }
            }
            let mut res = check(case, &got);
            #[cfg(feature = "ffi")]
            if res.is_ok() {
                res = check(case, &run(&|gids| via_abi(&input, gids)));
            }
            match res {
                Ok(()) => matched += 1,
                Err(e) => failures.push(format!("{id} {key}: {e}")),
            }
        }
    }
    eprintln!("matched {matched}");
    eprintln!("out of scope ({}):", unsupported.len());
    for u in &unsupported {
        eprintln!("  {u}");
    }
    assert!(
        failures.is_empty(),
        "parity failures:\n{}",
        failures.join("\n")
    );
    assert!(unsupported.is_empty(), "unsupported subset cases remain");
    assert!(
        matched >= 50,
        "only {matched} cases matched; goldens missing?"
    );
}
