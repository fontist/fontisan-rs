//! Golden iteration shared by the 1f-core parity tests (included with `#[path]`,
//! so `common/mod.rs` stays untouched). Goldens: `FONTISAN_GOLDENS_DIR`, else
//! `<corpus>/corpus/goldens`; the test skips with a message when they or the
//! fonts are absent (`FONTISAN_SPIKE_REQUIRE_CORPUS=1` turns a skip into a failure).
#![allow(dead_code)]
use super::*;
use serde_json::Value;
use std::path::PathBuf;

pub fn goldens_root() -> Option<PathBuf> {
    let dir = match std::env::var_os("FONTISAN_GOLDENS_DIR") {
        Some(d) => PathBuf::from(d),
        None => corpus_dir().join("corpus/goldens"),
    };
    if dir.join("read").is_dir() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("FONTISAN_SPIKE_REQUIRE_CORPUS").is_none(),
        "goldens missing at {}",
        dir.display()
    );
    eprintln!("SKIPPED: goldens not found at {}", dir.display());
    None
}

pub struct Case {
    pub id: String,
    /// The font file's basename (the audit's `source_file`).
    pub file: String,
    pub bytes: Vec<u8>,
    pub golden: Value,
    /// A `_swallow` case (only `full/eager` is recorded).
    pub swallow: bool,
}

fn json_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    v
}

/// Every ci-tier read golden plus the `_swallow` cases, with the font bytes.
pub fn cases(corpus: &std::path::Path, root: &std::path::Path) -> Vec<Case> {
    let mut out = Vec::new();
    let read = root.join("read");
    for path in json_files(&read) {
        let g: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if g["tier"] != "ci" {
            continue;
        }
        let file = g["file"].as_str().unwrap().to_string();
        out.push(Case {
            id: g["id"].as_str().unwrap().to_string(),
            bytes: std::fs::read(font_path(corpus, &file)).unwrap(),
            file,
            golden: g,
            swallow: false,
        });
    }
    for path in json_files(&read.join("_swallow")) {
        let g: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let font = path.with_extension("ttf");
        out.push(Case {
            id: format!("_swallow/{}", path.file_stem().unwrap().to_string_lossy()),
            file: font.file_name().unwrap().to_string_lossy().into_owned(),
            bytes: std::fs::read(&font).unwrap(),
            golden: g,
            swallow: true,
        });
    }
    out
}

/// A `read[<mode>/<lazy>]` entry, following `same_as`.
pub fn read_entry<'a>(g: &'a Value, key: &str) -> &'a Value {
    let e = &g["read"][key];
    match e["same_as"].as_str() {
        Some(s) => &g["read"][s],
        None => e,
    }
}
