//! 1d outline parity: `Font::outline_summary` against the read goldens
//! (`corpus/goldens/read/<id>.json`, card 0b) for glyf, CFF and CFF2, plus the
//! hand-built malformed cases for the outline tables (no golden covers them:
//! every read golden's glyf/loca/CFF/CFF2 parses cleanly), and a cross-check of
//! the raw glyph ranges against read-fonts.
mod common;

use common::*;
use fontisan::sfnt::{ErrorClass, Font, LoadingMode, Options, OutlineKind, face_count};
use serde_json::Value;
use std::path::PathBuf;

fn goldens_dir() -> Option<PathBuf> {
    let d = std::env::var_os("FONTISAN_READ_GOLDENS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("/Users/umerhassan/fontist/wt/fontisan-phase0/corpus/goldens/read")
        });
    if d.is_dir() {
        return Some(d);
    }
    if std::env::var_os("FONTISAN_SPIKE_REQUIRE_CORPUS").is_some() {
        panic!("read goldens missing at {}", d.display());
    }
    eprintln!("SKIPPED: read goldens not found at {}", d.display());
    None
}

fn resolve<'a>(read: &'a Value, key: &str) -> &'a Value {
    let v = &read[key];
    match v.get("same_as").and_then(Value::as_str) {
        Some(other) => &read[other],
        None => v,
    }
}

#[test]
fn outline_goldens_match_in_every_variant() {
    let (Some(corpus), Some(dir)) = (corpus(), goldens_dir()) else {
        return;
    };
    let mut fonts = 0;
    let mut faces_checked = 0;
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();
    for path in entries {
        let g: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let id = g["id"].as_str().unwrap().to_string();
        let bytes = std::fs::read(font_path(&corpus, g["file"].as_str().unwrap())).unwrap();
        // WOFF/WOFF2/dfont/Type 1 arrive with 1e; the sfnt model rejects them.
        let Some(n_faces) = face_count(&bytes) else {
            continue;
        };
        if Font::load(&bytes, 0, Options::default())
            .err()
            .is_some_and(|e| e.class == ErrorClass::Unsupported)
        {
            continue;
        }
        let mut seen = false;
        for (variant, mode, lazy) in [
            ("full/lazy", LoadingMode::Full, true),
            ("full/eager", LoadingMode::Full, false),
            ("metadata/lazy", LoadingMode::Metadata, true),
            ("metadata/eager", LoadingMode::Metadata, false),
        ] {
            let want = resolve(&g["read"], variant);
            let Some(faces) = want.get("ok").and_then(Value::as_array) else {
                continue; // an error golden belongs to 1c's read parity
            };
            assert_eq!(faces.len(), n_faces, "{id} {variant}: face count");
            for (i, face) in faces.iter().enumerate() {
                let font = Font::load(&bytes, i, Options { mode, lazy })
                    .unwrap_or_else(|e| panic!("{id}[{i}] {variant}: {e}"));
                let got = font.outline_summary();
                let w = &face["outlines"];
                assert_eq!(
                    got.kind.map(OutlineKind::as_str),
                    w["kind"].as_str(),
                    "{id}[{i}] {variant}: kind"
                );
                assert_eq!(
                    got.count as u64,
                    w["count"].as_u64().unwrap(),
                    "{id}[{i}] {variant}: count"
                );
                assert_eq!(
                    got.digest.as_deref(),
                    w["digest"].as_str(),
                    "{id}[{i}] {variant}: digest"
                );
                assert_eq!(got.error, None, "{id}[{i}] {variant}");
                if let Some(k) = got.kind {
                    *kinds.entry(k.as_str().to_string()).or_default() += 1;
                }
                faces_checked += 1;
                seen = true;
            }
        }
        fonts += usize::from(seen);
    }
    eprintln!(
        "outline parity: {fonts} fonts, {faces_checked} face/variant checks, kinds {kinds:?}"
    );
    assert!(
        fonts >= 25 && kinds.len() == 3,
        "too little covered: {fonts} {kinds:?}"
    );
}

/// The premise "read-fonts' glyf/loca and CFF/CFF2 INDEX access reproduce the
/// raw ranges" checked on the same corpus: every glyph's bytes equal read-fonts'.
#[test]
fn glyph_ranges_equal_read_fonts() {
    use read_fonts::tables::loca::Loca;
    use read_fonts::{FontRef, TableProvider, types::Tag};
    let Some(corpus) = corpus() else { return };
    let mut checked = 0;
    for name in [
        "SourceSans3-Regular.ttf",
        "NotoSans-Regular.ttf",
        "NotoSerifTangut-Regular.ttf",
        "SourceSans3VF-Upright.ttf",
        "SourceSans3-Regular.otf",
        "NotoSansJP-Regular.otf",
        "SourceSans3VF-Upright.otf",
        "TestTTF.ttf",
        "TestOTF.otf",
    ] {
        let bytes = std::fs::read(font_path(&corpus, name)).unwrap();
        let font = Font::from_bytes(&bytes).unwrap();
        let ours = font.outlines().unwrap().unwrap();
        let rf = FontRef::new(&bytes).unwrap();
        let theirs: Vec<&[u8]> = match ours.kind {
            OutlineKind::Glyf => {
                let loca: Loca = rf.loca(None).unwrap();
                let glyf = rf.table_data(Tag::new(b"glyf")).unwrap();
                let glyf = glyf.as_bytes();
                (0..ours.count())
                    .map(|g| {
                        let (a, b) = (
                            loca.get_raw(g).unwrap() as usize,
                            loca.get_raw(g + 1).unwrap() as usize,
                        );
                        &glyf[a.min(glyf.len())..b.min(glyf.len())]
                    })
                    .collect()
            }
            OutlineKind::Cff => {
                let c = read_fonts::ps::cff::CffFontRef::new_cff(
                    rf.table_data(Tag::new(b"CFF ")).unwrap().as_bytes(),
                    0,
                    None,
                )
                .unwrap();
                (0..c.charstrings().count() as usize)
                    .map(|g| c.charstrings().get(g).unwrap())
                    .collect()
            }
            OutlineKind::Cff2 => {
                let c = read_fonts::ps::cff::CffFontRef::new_cff2(
                    rf.table_data(Tag::new(b"CFF2")).unwrap().as_bytes(),
                    None,
                )
                .unwrap();
                (0..c.charstrings().count() as usize)
                    .map(|g| c.charstrings().get(g).unwrap())
                    .collect()
            }
        };
        assert_eq!(ours.glyphs, theirs, "{name}");
        checked += 1;
    }
    eprintln!("read-fonts cross-check: {checked} fonts equal");
}

// ---- malformed outline tables ------------------------------------------------
// No read golden has a malformed outline table, so these pin the error model
// (Ruby's class and message) from the Ruby sources instead.

/// A minimal sfnt with the given (tag, data) tables.
fn sfnt(version: u32, tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len();
    let mut out = version.to_be_bytes().to_vec();
    out.extend((n as u16).to_be_bytes());
    out.extend([0u8; 6]);
    let mut off = 12 + 16 * n;
    let mut body: Vec<u8> = vec![];
    for (tag, data) in tables {
        out.extend(*tag);
        out.extend([0u8; 4]);
        out.extend((off as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
        off += data.len();
    }
    out.extend(body);
    out
}

fn head(loc_format: i16) -> Vec<u8> {
    let mut h = vec![0u8; 54];
    h[50..52].copy_from_slice(&loc_format.to_be_bytes());
    h
}
fn maxp(n: u16) -> Vec<u8> {
    let mut m = vec![0, 0, 0x50, 0];
    m.extend(n.to_be_bytes());
    m
}

fn summary_of(bytes: &[u8]) -> fontisan::sfnt::OutlineSummary {
    Font::from_bytes(bytes).unwrap().outline_summary()
}

#[test]
fn glyf_short_and_long_loca_and_clipping() {
    // Two glyphs; the second runs past the end of glyf and the third starts past it.
    let glyf = b"ABCDEF".to_vec();
    let loca_long: Vec<u8> = [0u32, 4, 10, 20]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    let f = sfnt(
        0x00010000,
        &[
            (b"glyf", glyf.clone()),
            (b"head", head(1)),
            (b"loca", loca_long),
            (b"maxp", maxp(3)),
        ],
    );
    let o = Font::from_bytes(&f).unwrap().outlines().unwrap().unwrap();
    assert_eq!(o.glyphs, vec![&b"ABCD"[..], b"EF", b""]);
    let loca_short: Vec<u8> = [0u16, 1, 1, 2]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    let f = sfnt(
        0x00010000,
        &[
            (b"glyf", glyf),
            (b"head", head(0)),
            (b"loca", loca_short),
            (b"maxp", maxp(3)),
        ],
    );
    let o = Font::from_bytes(&f).unwrap().outlines().unwrap().unwrap();
    assert_eq!(o.glyphs, vec![&b"AB"[..], b"", b"CD"]);
}

#[test]
fn glyf_loca_errors_have_rubys_class_and_message() {
    let t = |loca: Vec<u8>, fmt, n| {
        let f = sfnt(
            0x00010000,
            &[
                (b"glyf", vec![0; 8]),
                (b"head", head(fmt)),
                (b"loca", loca),
                (b"maxp", maxp(n)),
            ],
        );
        Font::from_bytes(&f).unwrap().outlines().unwrap_err()
    };
    let e = t(vec![0, 0, 0, 1, 0, 2], 0, 3);
    assert_eq!(e.class, ErrorClass::CorruptedTable);
    assert_eq!(e.message, "Insufficient data for short offset at index 3");
    let long: Vec<u8> = [4u32, 2].iter().flat_map(|v| v.to_be_bytes()).collect();
    let e = t(long, 1, 1);
    assert_eq!(
        e.message,
        "Offsets are not monotonically increasing: offset[0]=4, offset[1]=2"
    );
    let e = t(vec![0; 4], 2, 1);
    assert_eq!(
        (e.class, e.message.as_str()),
        (
            ErrorClass::Argument,
            "indexToLocFormat must be 0 (short) or 1 (long), got: 2"
        )
    );
    let e = t(vec![0; 4], 0, 0);
    assert_eq!(e.message, "numGlyphs must be >= 1, got: 0");
    // The summary never raises: the class name lands in `error`.
    let f = sfnt(
        0x00010000,
        &[
            (b"glyf", vec![0; 8]),
            (b"head", head(0)),
            (b"loca", vec![0; 2]),
            (b"maxp", maxp(3)),
        ],
    );
    let s = summary_of(&f);
    assert_eq!(
        (s.kind, s.count, s.digest, s.error),
        (None, 0, None, Some(ErrorClass::CorruptedTable))
    );
}

fn cff_index(items: &[&[u8]]) -> Vec<u8> {
    let mut v = (items.len() as u16).to_be_bytes().to_vec();
    if items.is_empty() {
        return v;
    }
    v.push(2);
    let mut o = 1u16;
    v.extend(o.to_be_bytes());
    for i in items {
        o += i.len() as u16;
        v.extend(o.to_be_bytes());
    }
    for i in items {
        v.extend(*i);
    }
    v
}

fn cff1(charstrings: &[&[u8]]) -> Vec<u8> {
    let mut t = vec![1, 0, 4, 2];
    t.extend(cff_index(&[b"F"]));
    // Top DICT `29 <i32 offset> 17` (6 bytes), then empty String and Global Subr INDEXes.
    let off = t.len() + cff_index(&[&[0u8; 6]]).len() + 2 + 2;
    let mut dict = vec![29];
    dict.extend((off as i32).to_be_bytes());
    dict.push(17);
    t.extend(cff_index(&[&dict]));
    t.extend(cff_index(&[]));
    t.extend(cff_index(&[]));
    assert_eq!(t.len(), off);
    t.extend(cff_index(charstrings));
    t
}

#[test]
fn cff_charstrings_and_errors() {
    let data = cff1(&[b"\x0e", b"abc", b""]);
    let f = sfnt(0x4F54544F, &[(b"CFF ", data.clone())]);
    let o = Font::from_bytes(&f).unwrap().outlines().unwrap().unwrap();
    assert_eq!(
        (o.kind, o.glyphs),
        (OutlineKind::Cff, vec![&b"\x0e"[..], b"abc", b""])
    );
    // Truncate inside the CharStrings INDEX.
    let f = sfnt(0x4F54544F, &[(b"CFF ", data[..data.len() - 2].to_vec())]);
    let e = Font::from_bytes(&f).unwrap().outlines().unwrap_err();
    assert_eq!(e.class, ErrorClass::CorruptedTable);
    assert_eq!(
        e.message,
        "Failed to parse CharStrings INDEX: Unexpected end of INDEX data"
    );
    // A bad header.
    let mut bad = data;
    bad[0] = 9;
    let f = sfnt(0x4F54544F, &[(b"CFF ", bad)]);
    let e = Font::from_bytes(&f).unwrap().outlines().unwrap_err();
    assert_eq!(
        e.message,
        "Failed to parse CFF table: Invalid CFF header: version=9.0, hdr_size=4, off_size=2"
    );
}

fn cff2(charstrings: &[&[u8]], count_override: Option<u32>) -> Vec<u8> {
    // header(5) + top dict (29 <i32> 17 = 6 bytes) + global subr index (u32 count 0).
    let cs_off: i32 = 5 + 6 + 4;
    let mut t = vec![2, 0, 5, 0, 6, 29];
    t.extend(cs_off.to_be_bytes());
    t.push(17);
    t.extend(0u32.to_be_bytes());
    t.extend(
        count_override
            .unwrap_or(charstrings.len() as u32)
            .to_be_bytes(),
    );
    t.push(2);
    let mut o = 1u16;
    t.extend(o.to_be_bytes());
    for c in charstrings {
        o += c.len() as u16;
        t.extend(o.to_be_bytes());
    }
    for c in charstrings {
        t.extend(*c);
    }
    t
}

#[test]
fn cff2_charstrings_use_the_uint32_count_and_truncation_is_reported() {
    let data = cff2(&[b"xy", b"", b"z"], None);
    let f = sfnt(0x4F54544F, &[(b"CFF2", data.clone())]);
    let o = Font::from_bytes(&f).unwrap().outlines().unwrap().unwrap();
    assert_eq!(
        (o.kind, o.glyphs),
        (OutlineKind::Cff2, vec![&b"xy"[..], b"", b"z"])
    );
    let f = sfnt(0x4F54544F, &[(b"CFF2", cff2(&[b"xy"], Some(0)))]);
    assert_eq!(
        Font::from_bytes(&f)
            .unwrap()
            .outlines()
            .unwrap()
            .unwrap()
            .count(),
        0
    );
    let f = sfnt(0x4F54544F, &[(b"CFF2", data[..data.len() - 1].to_vec())]);
    let e = Font::from_bytes(&f).unwrap().outlines().unwrap_err();
    assert_eq!(e.class, ErrorClass::Truncated);
    let f = sfnt(0x4F54544F, &[(b"CFF2", data[..3].to_vec())]);
    assert_eq!(summary_of(&f).error, Some(ErrorClass::Truncated));
}

#[test]
fn selection_order_and_no_outlines() {
    // glyf+loca beat CFF; no outline table is not an error.
    let f = sfnt(0x00010000, &[(b"head", head(0)), (b"maxp", maxp(1))]);
    let s = summary_of(&f);
    assert_eq!((s.kind, s.count, s.digest, s.error), (None, 0, None, None));
    // glyf without loca falls through to CFF.
    let f = sfnt(
        0x4F54544F,
        &[(b"CFF ", cff1(&[b"q"])), (b"glyf", vec![1, 2])],
    );
    assert_eq!(summary_of(&f).kind, Some(OutlineKind::Cff));
}
