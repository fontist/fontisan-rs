//! 1b substrate spike, malformed set: does `read-fonts` reproduce the points
//! where Ruby fails? Ruby (`scripts/spike-expected.rb`, section "malformed")
//! rejects eagerly only when the table directory itself is unreadable; bad
//! offsets, bad checksums and truncated table bodies load and fail on later
//! access. An eager reject in Rust where Ruby loads counts as a mismatch.
mod common;

use common::*;
use read_fonts::types::Tag;
use read_fonts::{FontRef, ReadError, TableProvider};

const PARSED: [&str; 7] = ["head", "hhea", "maxp", "OS/2", "post", "cmap", "name"];

fn tag(t: &str) -> Tag {
    Tag::new_checked(format!("{t:<4}").as_bytes()).unwrap()
}

/// "ok", "absent" (no table data) or "error" for the Rust access of one table.
fn rust_parse(font: &FontRef, t: &str) -> &'static str {
    let r: Result<(), ReadError> = match t {
        "head" => font.head().map(|_| ()),
        "hhea" => font.hhea().map(|_| ()),
        "maxp" => font.maxp().map(|_| ()),
        "OS/2" => font.os2().map(|_| ()),
        "post" => font.post().map(|_| ()),
        "cmap" => font.cmap().map(|_| ()),
        "name" => font.name().map(|_| ()),
        _ => unreachable!(),
    };
    match r {
        Ok(()) => "ok",
        Err(ReadError::TableIsMissing(_)) => "absent",
        Err(_) => "error",
    }
}

/// FINDING: `FontRef::new` does not check that the declared table records fit in
/// the file (bad-num-tables and truncated-100 load), where Ruby rejects at load. The adapter adds
/// this one explicit guard; it is what turns the load verdict into a match.
fn directory_fits(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && 12 + 16 * u16::from_be_bytes([bytes[4], bytes[5]]) as usize <= bytes.len()
}

#[test]
fn every_malformed_kind_matches_ruby_failure_points() {
    let Some(corpus) = corpus() else { return };
    let exp = expected();
    let kinds = exp["malformed"].as_object().unwrap();
    assert_eq!(kinds.len(), 8, "the malformed set has 8 kinds");
    let mut partial_differences = Vec::new();
    let mut eager_gaps: Vec<String> = Vec::new();
    for (file, want) in kinds {
        let bytes = std::fs::read(corpus.join("derived").join(file)).unwrap();
        let loaded = FontRef::new(&bytes);
        // load verdict: Ruby rejects (any class) <=> read-fonts rejects, plus the guard.
        let guarded_ok = loaded.is_ok() && directory_fits(&bytes);
        assert_eq!(
            want["load"] == "ok",
            guarded_ok,
            "{file}: ruby load={} rust load_ok={}",
            want["load"],
            guarded_ok
        );
        if loaded.is_ok() != guarded_ok {
            eager_gaps.push(file.clone());
        }
        let loaded = if guarded_ok { loaded } else { continue };
        let Ok(font) = loaded else { continue };
        let data = want["data"].as_object().unwrap();
        for (t, rb) in data {
            let rust = font.table_data(tag(t)).map(|d| d.len() as u64);
            match (rb.as_u64(), rust) {
                (None, None) => {}
                (Some(a), Some(b)) => assert_eq!(a, b, "{file}/{t}: table length"),
                // Ruby hands back the short bytes of a table that runs past EOF;
                // read-fonts hands back nothing. Only the truncated body may differ.
                (Some(a), None) => {
                    let declared = font
                        .table_directory
                        .table_records()
                        .iter()
                        .find(|r| r.tag() == tag(t))
                        .map(|r| r.length() as u64)
                        .unwrap();
                    assert!(
                        a < declared,
                        "{file}/{t}: ruby read {a} of {declared} but rust has none"
                    );
                    partial_differences.push(format!(
                        "{file}/{t}: ruby {a} of {declared} bytes, rust None"
                    ));
                }
                (None, Some(b)) => panic!("{file}/{t}: ruby nil but rust has {b} bytes"),
            }
        }
        for t in PARSED {
            let ruby = want["parse"][t].as_str().unwrap();
            let rust = rust_parse(&font, t);
            let ruby_has_data = data[t].as_u64().is_some();
            match ruby {
                "ok" if ruby_has_data => assert_eq!(rust, "ok", "{file}/{t}"),
                "ok" => assert_eq!(
                    rust, "absent",
                    "{file}/{t}: ruby treats missing data as absent"
                ),
                _ => assert_ne!(
                    rust, "ok",
                    "{file}/{t}: ruby fails ({ruby}) but rust parsed"
                ),
            }
        }
    }
    // The one known difference is the partial `name` body of truncated-half.
    eprintln!("partial-table differences: {partial_differences:?}");
    assert_eq!(partial_differences.len(), 1);
    eprintln!("read-fonts alone loads (needs the directory guard): {eager_gaps:?}");
    assert_eq!(
        eager_gaps,
        [
            "malformed-bad-num-tables.ttf",
            "malformed-truncated-100.ttf"
        ]
    );
}

/// read-fonts does not validate per-table or head checkSumAdjustment checksums,
/// which is what Ruby does too: the bad-checksum font parses all the way.
#[test]
fn bad_checksum_font_parses_fully() {
    let Some(corpus) = corpus() else { return };
    let bytes = std::fs::read(corpus.join("derived/malformed-bad-checksum.ttf")).unwrap();
    let font = FontRef::new(&bytes).unwrap();
    assert_eq!(font.head().unwrap().checksum_adjustment(), 0x1234_5678);
    assert!(font.glyf().is_ok() && font.cmap().is_ok());
}

/// The failure point of truncated-half is the first access of `name`, not the load.
#[test]
fn truncated_half_fails_at_name_access_not_at_load() {
    let Some(corpus) = corpus() else { return };
    let bytes = std::fs::read(corpus.join("derived/malformed-truncated-half.ttf")).unwrap();
    let font = FontRef::new(&bytes).expect("loads lazily, like Ruby");
    assert!(font.head().is_ok() && font.cmap().is_ok());
    assert!(font.name().is_err());
}

/// The guard never rejects a valid font from the corpus.
#[test]
fn directory_guard_accepts_every_valid_corpus_font() {
    let Some(corpus) = corpus() else { return };
    for name in fonts_with_ext(&corpus) {
        let bytes = std::fs::read(&name).unwrap();
        assert!(directory_fits(&bytes), "{}", name.display());
        assert!(FontRef::new(&bytes).is_ok(), "{}", name.display());
    }
}

fn fonts_with_ext(corpus: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    for d in ["fonts", "derived"] {
        for e in std::fs::read_dir(corpus.join(d)).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if (n.ends_with(".ttf") || n.ends_with(".otf")) && !n.starts_with("malformed") {
                v.push(e.path());
            }
        }
    }
    v.sort();
    assert!(v.len() >= 20);
    v
}
