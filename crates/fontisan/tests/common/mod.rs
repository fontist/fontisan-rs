//! Shared helpers for the 1b substrate spike tests (not production code).
#![allow(dead_code)]
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn corpus_dir() -> PathBuf {
    match std::env::var_os("FONTISAN_CORPUS_DIR") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.corpus/fontisan"),
    }
}

/// `corpus/` inside the fetched fontisan checkout, or None when it is absent.
/// CI does not run `cargo xtask corpus-fetch` yet, so a missing corpus skips
/// (and says so); `FONTISAN_SPIKE_REQUIRE_CORPUS=1` turns the skip into a failure.
pub fn corpus() -> Option<PathBuf> {
    let c = corpus_dir().join("corpus");
    if c.join("fonts").is_dir() {
        return Some(c);
    }
    if std::env::var_os("FONTISAN_SPIKE_REQUIRE_CORPUS").is_some() {
        panic!(
            "corpus missing at {}; run `cargo xtask corpus-fetch`",
            c.display()
        );
    }
    eprintln!(
        "SKIPPED: corpus not fetched ({}); run `cargo xtask corpus-fetch`",
        c.display()
    );
    None
}

pub fn font_path(corpus: &Path, name: &str) -> PathBuf {
    for d in ["fonts", "derived"] {
        let p = corpus.join(d).join(name);
        if p.exists() {
            return p;
        }
    }
    panic!("{name} not in corpus/fonts or corpus/derived");
}

pub fn expected() -> Value {
    serde_json::from_str(include_str!("../spike/expected.json")).unwrap()
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

pub fn sha_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
