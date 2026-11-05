//! 1e-wire parity: WOFF, WOFF2 and WOFF2 collections read through
//! `sfnt::Font::load` (decode, then the 1c model) against the read goldens'
//! `format`, `tables`, `header_fields`, `cmap`, `names`, `post`, `variation`
//! and `fvar`, in all four mode/lazy combinations and for every collection member.
//!
//! Goldens: `FONTISAN_GOLDENS_DIR/read/*.json`, else `<corpus>/corpus/goldens/read`.
//! Skips with a message when goldens or fonts are absent;
//! `FONTISAN_SPIKE_REQUIRE_CORPUS=1` turns a skip into a failure.
mod common;

use common::variation_json::{fvar, variation};
use common::*;
use fontisan::sfnt::{ErrorClass, Font, Format, LoadingMode, Options};
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

const WOFF_FORMATS: [&str; 3] = ["woff", "woff2", "woff2_collection"];
const EXPECTED_IDS: [&str; 7] = [
    "fonttools-test-woff",
    "fonttools-test-woff2",
    "source-sans-3-regular-otf-woff",
    "source-sans-3-regular-otf-woff2",
    "source-sans-3-regular-ttf-woff",
    "source-sans-3-regular-ttf-woff2",
    "woff2-collection-4members",
];

#[test]
fn woff_goldens_match_in_all_modes_and_members() {
    let (Some(corpus), Some(dir)) = (corpus(), goldens_dir()) else {
        return;
    };
    let mut ids = Vec::new();
    let mut checks = 0;
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for path in files {
        let g: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let fmt = g["read"]["metadata/lazy"]["ok"][0]["format"].as_str();
        let fmt = fmt.or_else(|| g["read"]["full/lazy"]["ok"][0]["format"].as_str());
        if !fmt.is_some_and(|f| WOFF_FORMATS.contains(&f)) {
            continue;
        }
        let id = g["id"].as_str().unwrap().to_string();
        let bytes = std::fs::read(font_path(&corpus, g["file"].as_str().unwrap())).unwrap();
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
            let mut want = &g["read"][&gk];
            if let Some(s) = want["same_as"].as_str() {
                want = &g["read"][s];
            }
            let faces = want["ok"]
                .as_array()
                .unwrap_or_else(|| panic!("{id} {gk}: not ok"));
            for (i, face) in faces.iter().enumerate() {
                let font =
                    Font::load(&bytes, i, opts).unwrap_or_else(|e| panic!("{id}[{i}] {gk}: {e}"));
                let (got, extra) =
                    read_face(&bytes, &font).unwrap_or_else(|e| panic!("{id}[{i}] {gk}: {e}"));
                for k in ["format", "file_sha256", "tables", "header_fields", "cmap"] {
                    assert_eq!(got[k], face[k], "{id}[{i}] {gk}: {k}");
                }
                assert_eq!(extra["names"], face["names"], "{id}[{i}] {gk}: names");
                let post = &extra["post_glyph_names"];
                assert_eq!(
                    post["count"], face["post"]["glyph_name_count"],
                    "{id}[{i}] {gk}: post count"
                );
                assert_eq!(
                    post["sha256"], face["post"]["glyph_names_sha256"],
                    "{id}[{i}] {gk}: post names"
                );
                assert_eq!(
                    variation(&font),
                    face["variation"],
                    "{id}[{i}] {gk}: variation"
                );
                assert_eq!(fvar(&font), face["fvar"], "{id}[{i}] {gk}: fvar");
                checks += 1;
            }
        }
        ids.push(id);
    }
    ids.sort();
    assert_eq!(ids, EXPECTED_IDS, "WOFF golden set changed");
    eprintln!(
        "woff read parity: {} goldens, {checks} face/mode checks",
        ids.len()
    );
}

#[test]
fn collection_member_out_of_range_and_garbage_are_errors_not_panics() {
    let Some(corpus) = corpus() else { return };
    let bytes = std::fs::read(font_path(&corpus, "woff2-collection-4members.woff2")).unwrap();
    let e = Font::load(&bytes, 9, Options::default()).err().unwrap();
    assert_eq!(e.class, ErrorClass::InvalidFont);
    assert_eq!(
        e.message,
        "Font index 9 out of range (collection has 4 fonts)"
    );
    let f = Font::load(&bytes, 3, Options::default()).unwrap();
    assert_eq!(f.format(), Format::Woff2Collection);
    // a WOFF2 signature over garbage is rejected by the decoder
    let mut junk = b"wOF2".to_vec();
    junk.extend([0xAB; 40]);
    assert!(Font::from_bytes(&junk).is_err());
    let mut junk = b"wOFF".to_vec();
    junk.extend([0xAB; 40]);
    assert!(
        Font::load(
            &junk,
            0,
            Options {
                mode: LoadingMode::Metadata,
                lazy: true
            }
        )
        .is_err()
    );
}

/// Damaged WOFF2 input never panics: every truncation length and a byte flip
/// at spread positions either loads or returns an `Error`.
#[test]
fn damaged_woff2_never_panics() {
    let Some(corpus) = corpus() else { return };
    for name in ["woff2-collection-4members.woff2", "TestWOFF2.woff2"] {
        let bytes = std::fs::read(font_path(&corpus, name)).unwrap();
        for len in 0..bytes.len().min(400) {
            let _ = Font::load(&bytes[..len], 0, Options::default());
        }
        for at in (0..bytes.len()).step_by(7) {
            let mut b = bytes.clone();
            b[at] ^= 0xff;
            for i in 0..2 {
                let _ = Font::load(&b, i, Options::default());
            }
        }
    }
}
