//! Shared helpers for the 1b substrate spike tests (not production code).
#![allow(dead_code)]
pub mod variation_json;

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

// ---- 1c: sfnt model parity helpers -------------------------------------

use fontisan::sfnt::{Font, Options, Value as V};

pub fn snapshot_json(s: &[(&'static str, V)]) -> Value {
    fn v(x: &V) -> Value {
        match x {
            V::Int(i) => Value::from(*i),
            V::Text(t) => Value::from(t.clone()),
            V::List(l) => Value::Array(l.iter().map(v).collect()),
        }
    }
    Value::Object(s.iter().map(|(k, x)| (k.to_string(), v(x))).collect())
}

/// `Font::load` over a corpus file.
pub fn load(bytes: &[u8], index: usize, opts: Options) -> Result<Font<'_>, fontisan::sfnt::Error> {
    Font::load(bytes, index, opts)
}

pub fn combos() -> [(&'static str, Options); 4] {
    use fontisan::sfnt::LoadingMode::{Full, Metadata};
    [
        (
            "full/lazy=false",
            Options {
                mode: Full,
                lazy: false,
            },
        ),
        (
            "full/lazy=true",
            Options {
                mode: Full,
                lazy: true,
            },
        ),
        (
            "metadata/lazy=false",
            Options {
                mode: Metadata,
                lazy: false,
            },
        ),
        (
            "metadata/lazy=true",
            Options {
                mode: Metadata,
                lazy: true,
            },
        ),
    ]
}

/// A table that is gated out by the mode reads as absent, like the facade's `available_table`.
fn gated<T>(
    r: Result<Option<T>, fontisan::sfnt::Error>,
) -> Result<Option<T>, fontisan::sfnt::Error> {
    match r {
        Err(e) if e.class == fontisan::sfnt::ErrorClass::Argument => Ok(None),
        other => other,
    }
}

/// The 1c slice of `Fontisan::Reference.read` for one face (format, tables,
/// names, header_fields, cmap, post glyph names), or the first error read raises.
pub fn read_face(bytes: &[u8], font: &Font) -> Result<(Value, Value), fontisan::sfnt::Error> {
    let tables: Vec<Value> = font
        .table_summaries()
        .into_iter()
        .map(|t| {
            let sha = t.length.and_then(|_| font.table_data(&t.tag)).map(sha_hex);
            serde_json::json!({"tag": t.tag, "length": t.length, "sha256": sha,
                               "error": t.error.map(|c| c.ruby_name())})
        })
        .collect();
    let names: Vec<Value> = match gated(font.name())? {
        None => vec![],
        Some(n) => n
            .records
            .iter()
            .map(|r| {
                serde_json::json!({"platform_id": r.platform_id, "encoding_id": r.encoding_id,
                    "language_id": r.language_id, "name_id": r.name_id, "string": n.decode(r)})
            })
            .collect(),
    };
    let snap = |s: Option<Vec<(&'static str, V)>>| s.map_or(Value::Null, |s| snapshot_json(&s));
    let hf = serde_json::json!({
        "head": snap(gated(font.head())?.map(|t| t.snapshot())),
        "hhea": snap(gated(font.hhea())?.map(|t| t.snapshot())),
        "maxp": snap(gated(font.maxp())?.map(|t| t.snapshot())),
        "OS/2": snap(gated(font.os2())?.map(|t| t.snapshot())),
    });
    let cmap = match gated(font.cmap())? {
        None => Value::Null,
        Some(c) => {
            let cps = c.codepoints();
            let be: Vec<u8> = cps.iter().flat_map(|c| c.to_be_bytes()).collect();
            serde_json::json!({"count": cps.len(), "sha256": sha_hex(&be), "subtable_formats": c.subtable_formats()})
        }
    };
    let post = match gated(font.post())? {
        None => Value::Null,
        Some(p) => {
            let g = p.glyph_names();
            serde_json::json!({"count": g.len(), "sha256": sha_hex(g.join("\n").as_bytes())})
        }
    };
    Ok((
        serde_json::json!({"format": font.format().as_str(), "file_sha256": sha_hex(bytes),
                           "tables": tables, "header_fields": hf, "cmap": cmap}),
        serde_json::json!({"names": names, "post_glyph_names": post}),
    ))
}
