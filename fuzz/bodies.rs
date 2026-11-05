//! Fuzz target bodies, shared by the cargo-fuzz targets (`fuzz/fuzz_targets/*`)
//! and the stable mutation test (`crates/fontisan/tests/fuzz_mutation.rs`), so the
//! harness code is exercised on every `cargo test`, not only in the nightly job.
//!
//! Contract of every body: any `&[u8]` returns normally (errors are fine, a panic
//! or hang is the finding).
#![allow(dead_code)]

use fontisan::sfnt::{Font, LoadingMode, Options, face_count};
use fontisan::{audit, woff};

/// Faces tried per input; a collection header can claim thousands.
const MAX_FACES: usize = 4;

const ALL_OPTIONS: [Options; 4] = [
    Options {
        mode: LoadingMode::Full,
        lazy: false,
    },
    Options {
        mode: LoadingMode::Full,
        lazy: true,
    },
    Options {
        mode: LoadingMode::Metadata,
        lazy: false,
    },
    Options {
        mode: LoadingMode::Metadata,
        lazy: true,
    },
];

/// Run `f` on every face of `data` in every loading mode.
fn each_font(data: &[u8], mut f: impl FnMut(&Font<'_>)) {
    let faces = face_count(data).unwrap_or(1).min(MAX_FACES);
    for index in 0..faces.max(1) {
        for options in ALL_OPTIONS {
            if let Ok(font) = Font::load(data, index, options) {
                f(&font);
            }
        }
    }
}

/// `Font::load` in all modes and faces, plus the directory view.
pub fn font_load(data: &[u8]) {
    each_font(data, |font| {
        let _ = (font.format(), font.directory_fields(), font.table_names());
        for e in font.entries() {
            let _ = (
                e.tag_str(),
                font.has_table(&e.tag_str()),
                font.table_data(&e.tag_str()),
            );
        }
        let _ = font.table_summaries();
        let _ = font.woff2_header();
    });
}

/// Fixed-layout and name/cmap/post table accessors.
pub fn font_tables(data: &[u8]) {
    each_font(data, |font| {
        let _ = (font.head(), font.hhea(), font.maxp(), font.os2());
        if let Ok(Some(post)) = font.post() {
            let _ = post.glyph_names();
        }
        if let Ok(Some(name)) = font.name() {
            for r in &name.records {
                let _ = name.decode(r);
            }
            for id in 0..=25 {
                let _ = name.english_name(id);
            }
        }
        if let Ok(Some(cmap)) = font.cmap() {
            let _ = (cmap.subtable_formats(), cmap.codepoints());
        }
    });
}

/// glyf/loca, CFF and CFF2 outline parsing.
pub fn font_outlines(data: &[u8]) {
    each_font(data, |font| {
        if let Ok(Some(o)) = font.outlines() {
            let _ = (o.count(), o.hashes(), o.digest());
        }
        let _ = font.outline_summary();
        let _ = font.cff_charstring_count();
    });
}

/// fvar, avar, gvar, HVAR, VVAR, MVAR, STAT.
pub fn font_variations(data: &[u8]) {
    each_font(data, |font| {
        let _ = (font.fvar(), font.avar());
        let _ = (font.gvar_digest(), font.hvar_digest(), font.vvar_digest());
        let _ = (font.mvar_digest(), font.stat_digest());
    });
}

/// GSUB/GPOS script and feature walk; the raw input is also parsed as a table body.
pub fn font_layout(data: &[u8]) {
    let walk = |t: &fontisan::sfnt::LayoutTable| {
        for script in t.scripts() {
            let _ = t.features(&script);
        }
    };
    if let Ok(t) = fontisan::sfnt::LayoutTable::from_bytes(data) {
        walk(&t);
    }
    each_font(data, |font| {
        for t in [font.gsub(), font.gpos()].into_iter().flatten().flatten() {
            walk(&t);
        }
    });
}

/// Decode output must itself be loadable without panicking.
fn load_decoded(decoded: &[u8]) {
    font_load(decoded);
}

/// WOFF 1 decode.
pub fn woff1(data: &[u8]) {
    if let Ok(sfnt) = woff::decode_woff(data) {
        load_decoded(&sfnt);
    }
}

/// WOFF2 (single font) decode, both padding flavours.
pub fn woff2(data: &[u8]) {
    if let Ok(sfnt) = woff::decode_woff2(data) {
        load_decoded(&sfnt);
    }
    let _ = woff::decode_woff2_ruby_padding(data);
}

/// WOFF2 collection decode.
pub fn woff2_collection(data: &[u8]) {
    let _ = woff::is_woff2_collection(data);
    if let Ok(ttc) = woff::decode_woff2_collection(data) {
        load_decoded(&ttc);
    }
}

/// Every registered audit check on every loadable face.
pub fn audit_loaded(data: &[u8]) {
    each_font(data, |font| {
        let _ = audit::run_checks(font);
    });
}

/// (name, body) for every target; the mutation test iterates this, and its length
/// is compared against `fuzz/fuzz_targets/` so a target cannot be left untested.
pub type Body = fn(&[u8]);
pub const TARGETS: &[(&str, Body)] = &[
    ("font_load", font_load),
    ("font_tables", font_tables),
    ("font_outlines", font_outlines),
    ("font_variations", font_variations),
    ("font_layout", font_layout),
    ("woff1", woff1),
    ("woff2", woff2),
    ("woff2_collection", woff2_collection),
    ("audit_loaded", audit_loaded),
];
