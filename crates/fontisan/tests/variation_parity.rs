//! 1d-var parity: fvar and the gvar/HVAR/VVAR/MVAR/STAT digests against the
//! `variation` and `fvar` fields of the read goldens, every ci-tier font, in all
//! four mode/lazy combinations (metadata mode gates the tables out: all null).
//!
//! Goldens: `FONTISAN_GOLDENS_DIR/read/*.json`, else `<corpus>/corpus/goldens/read`.
//! The test skips with a message when goldens or fonts are absent;
//! `FONTISAN_SPIKE_REQUIRE_CORPUS=1` turns a skip into a failure.
mod common;

use common::variation_json::{fvar, variation};
use common::*;
use fontisan::sfnt::*;
use serde_json::Value;
use std::path::PathBuf;

fn goldens_dir() -> Option<PathBuf> {
    let dir = match std::env::var_os("FONTISAN_GOLDENS_DIR") {
        Some(d) => PathBuf::from(d),
        None => corpus_dir().join("corpus/goldens"),
    };
    let read = dir.join("read");
    if read.is_dir() {
        return Some(read);
    }
    assert!(
        std::env::var_os("FONTISAN_SPIKE_REQUIRE_CORPUS").is_none(),
        "read goldens missing at {}",
        read.display()
    );
    eprintln!("SKIPPED: read goldens not found at {}", read.display());
    None
}

#[test]
fn variation_fields_match_read_goldens_on_every_ci_font() {
    let (Some(corpus), Some(dir)) = (corpus(), goldens_dir()) else {
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let (mut fonts, mut variable, mut checks) = (0, 0, 0);
    let mut present = std::collections::BTreeMap::new();
    for path in files {
        let g: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if g["tier"] != "ci" {
            continue;
        }
        let file = g["file"].as_str().unwrap();
        let Some(faces) = g["read"]["full/lazy"]["ok"].as_array() else {
            continue; // unsupported container or expected error
        };
        if !matches!(
            g["read"]["full/lazy"]["ok"][0]["format"].as_str(),
            Some("ttf" | "otf" | "ttc" | "otc")
        ) {
            continue;
        }
        let bytes = std::fs::read(font_path(&corpus, file)).unwrap();
        fonts += 1;
        for (i, _) in faces.iter().enumerate() {
            for (combo, opts) in combos() {
                let gk = format!(
                    "{}/{}",
                    if combo.starts_with("full") {
                        "full"
                    } else {
                        "metadata"
                    },
                    if combo.ends_with("true") {
                        "lazy"
                    } else {
                        "eager"
                    }
                );
                let want = &g["read"][&gk];
                let want = if let Some(s) = want["same_as"].as_str() {
                    &g["read"][s]
                } else {
                    want
                };
                let want = &want["ok"][i];
                let font = Font::load(&bytes, i, opts).unwrap();
                let id = g["id"].as_str().unwrap();
                assert_eq!(
                    variation(&font),
                    want["variation"],
                    "{id}[{i}] {gk}: variation"
                );
                assert_eq!(fvar(&font), want["fvar"], "{id}[{i}] {gk}: fvar");
                checks += 1;
                if combo == "full/lazy=true" {
                    for t in ["gvar", "HVAR", "VVAR", "MVAR", "STAT"] {
                        if !want["variation"][t].is_null() {
                            *present.entry(t).or_insert(0) += 1;
                        }
                    }
                }
                if combo == "full/lazy=true" && !want["fvar"].is_null() {
                    variable += 1;
                }
            }
        }
    }
    eprintln!("variation parity: {fonts} fonts, {variable} variable faces, {checks} checks");
    eprintln!("faces with digest: {present:?}");
    assert!(fonts >= 25 && variable >= 5, "too few: {fonts}/{variable}");
    // No ci-tier font has a VVAR (every golden is null): that digest shares the
    // HVAR code path but is not golden-exercised.
    for t in ["gvar", "HVAR", "MVAR", "STAT"] {
        assert!(
            present.get(t).copied().unwrap_or(0) >= 1,
            "{t} never exercised"
        );
    }
}

#[test]
fn avar_segment_maps_cover_every_fvar_axis() {
    let Some(corpus) = corpus() else { return };
    let mut seen = 0;
    for name in [
        "SourceSans3VF-Upright.ttf",
        "SourceSans3VF-Italic.ttf",
        "MonaSansVF[wdth,wght,opsz,ital].ttf",
    ] {
        let bytes = std::fs::read(font_path(&corpus, name)).unwrap();
        let f = Font::from_bytes(&bytes).unwrap();
        if let Some(avar) = f.avar().unwrap() {
            let axes = f.fvar().unwrap().unwrap().axes.len();
            assert_eq!(avar.segment_maps.len(), axes, "{name}");
            // each map holds the -1/0/1 anchors
            for m in &avar.segment_maps {
                assert!(m.contains(&(0, 0)), "{name}: {m:?}");
            }
            seen += 1;
        }
    }
    eprintln!("avar: {seen} fonts with avar");
}

/// A table cut short records the facade's Truncated failure; a flipped byte in
/// a digested range changes the digest (the parity check is not vacuous).
#[test]
fn truncated_and_altered_tables_are_detected() {
    let Some(corpus) = corpus() else { return };
    let bytes = std::fs::read(font_path(&corpus, "SourceSans3VF-Upright.ttf")).unwrap();
    let f = Font::from_bytes(&bytes).unwrap();
    let gvar = f.table_data("gvar").unwrap();
    let want = f.gvar_digest().unwrap().unwrap();
    // locate the table inside the file by its directory entry
    let e = f.entries().iter().find(|e| e.tag_str() == "gvar").unwrap();
    let start = e.offset as usize;
    let mut cut = bytes.clone();
    cut.truncate(start + 40);
    // keep the directory length as is: the table now runs past EOF and is short
    let g = Font::from_bytes(&cut).unwrap();
    assert_eq!(g.gvar_digest(), Err(DigestFailure));
    let mut flipped = bytes.clone();
    flipped[start + gvar.len() - 1] ^= 0xff;
    let h = Font::from_bytes(&flipped).unwrap();
    let got = h.gvar_digest().unwrap().unwrap();
    assert_ne!(got.glyphs_digest, want.glyphs_digest);
}
