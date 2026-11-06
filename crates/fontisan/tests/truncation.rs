//! 1c lazy-failure surface: every 1c table cut to N bytes must fail (or not)
//! exactly as the Ruby reference does, with the same error class. Expected
//! values: `read/truncation.json` from `scripts/truncation-expected.rb`.
//! Also measures why the fixed-layout tables are not delegated to read-fonts.
mod common;

use common::*;
use fontisan::sfnt::Font;
use read_fonts::{FontRef, TableProvider};
use serde_json::Value;

/// The same sequential, 4-byte-aligned sfnt the Ruby script builds.
fn build(mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by_key(|t| t.0);
    let n = tables.len();
    let mut off = 12 + 16 * n;
    let mut dir = Vec::new();
    let mut body = Vec::new();
    for (tag, d) in &tables {
        dir.extend_from_slice(tag);
        dir.extend_from_slice(&0u32.to_be_bytes());
        dir.extend_from_slice(&((off + body.len()) as u32).to_be_bytes());
        dir.extend_from_slice(&(d.len() as u32).to_be_bytes());
        body.extend_from_slice(d);
        body.resize(body.len() + (4 - d.len() % 4) % 4, 0);
    }
    let _ = &mut off;
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    out.extend(dir);
    out.extend(body);
    out
}

fn tag4(t: &str) -> [u8; 4] {
    let mut a = [b' '; 4];
    a[..t.len()].copy_from_slice(t.as_bytes());
    a
}

fn rust_outcome(font: &Font, tag: &str) -> String {
    let r = match tag {
        "head" => font.head().map(|_| ()),
        "hhea" => font.hhea().map(|_| ()),
        "maxp" => font.maxp().map(|_| ()),
        "OS/2" => font.os2().map(|_| ()),
        "post" => font.post().map(|p| {
            p.map(|p| p.glyph_names());
        }),
        "name" => font.name().map(|n| {
            if let Some(n) = n {
                n.records.iter().for_each(|r| drop(n.decode(r)));
            }
        }),
        "cmap" => font.cmap().map(|c| {
            if let Some(c) = c {
                drop(c.codepoints());
            }
        }),
        _ => unreachable!(),
    };
    r.map_or_else(|e| e.class.ruby_name().to_string(), |_| "ok".to_string())
}

fn read_fonts_ok(bytes: &[u8], tag: &str) -> bool {
    let Ok(f) = FontRef::new(bytes) else {
        return false;
    };
    match tag {
        "head" => f.head().is_ok(),
        "hhea" => f.hhea().is_ok(),
        "maxp" => f.maxp().is_ok(),
        "OS/2" => f.os2().is_ok(),
        "post" => f.post().is_ok(),
        "name" => f.name().is_ok(),
        "cmap" => f.cmap().is_ok(),
        _ => unreachable!(),
    }
}

#[test]
fn truncated_table_bodies_fail_like_ruby() {
    let Some(corpus) = corpus() else { return };
    let exp: Value = serde_json::from_str(include_str!("read/truncation.json")).unwrap();
    let (mut checked, mut rf_diverges) = (0, 0);
    for (src, tables) in exp.as_object().unwrap() {
        let bytes = std::fs::read(font_path(&corpus, src)).unwrap();
        let base = Font::from_bytes(&bytes).unwrap();
        let all: Vec<([u8; 4], Vec<u8>)> = base
            .entries()
            .iter()
            .map(|e| (e.tag, base.table_data(&e.tag_str()).unwrap().to_vec()))
            .collect();
        for (tag, lens) in tables.as_object().unwrap() {
            let t4 = tag4(tag);
            let full = &all.iter().find(|t| t.0 == t4).unwrap().1;
            assert_eq!(
                lens["full_length"].as_u64().unwrap() as usize,
                full.len(),
                "{src} {tag}"
            );
            for (len, want) in lens
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| *k != "full_length")
            {
                let n: usize = len.parse().unwrap();
                let mut tabs = all.clone();
                tabs.iter_mut().find(|t| t.0 == t4).unwrap().1 = full[..n].to_vec();
                let font_bytes = build(tabs);
                let font = Font::from_bytes(&font_bytes).unwrap();
                let got = rust_outcome(&font, tag);
                assert_eq!(&got, want.as_str().unwrap(), "{src} {tag} cut to {n}");
                if read_fonts_ok(&font_bytes, tag) != (got == "ok") {
                    rf_diverges += 1;
                }
                checked += 1;
            }
        }
    }
    eprintln!(
        "truncation: {checked} cases match Ruby; read-fonts' accept/reject differs from Ruby in {rf_diverges}"
    );
    assert!(checked > 300);
    // Evidence for the substrate verdict: read-fonts cannot stand in for the strict readers.
    assert!(rf_diverges > 0);
}
