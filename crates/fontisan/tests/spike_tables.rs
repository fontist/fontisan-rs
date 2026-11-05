//! 1b substrate spike: can `read-fonts` / `skrifa` reproduce the Ruby facade
//! for cmap, glyf and gvar? Expected values come from `Fontisan::Reference`
//! (cmap, outlines) and the Ruby glyf/gvar parsers, via
//! `scripts/spike-expected.rb` (goldens did not exist when the spike ran).
//! Dev-only: nothing here is a production adapter.
mod common;

use common::*;
use read_fonts::Offset;
use read_fonts::tables::glyf::{Glyph, PointFlags};
use read_fonts::types::Point;
use read_fonts::types::{GlyphId, Tag};
use read_fonts::{FontRef, TableProvider};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn open(corpus: &std::path::Path, name: &str) -> Vec<u8> {
    std::fs::read(font_path(corpus, name)).unwrap()
}

/// Names (in expected.json) of fonts that carry the given expected key.
fn fonts_with(exp: &Value, key: &str) -> Vec<String> {
    exp["fonts"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, v)| v.get(key).is_some_and(|x| !x.is_null()))
        .map(|(k, _)| k.clone())
        .collect()
}

// ---------------------------------------------------------------- cmap

/// Ruby's `Cmap#unicode_mappings`: ONE subtable, chosen (3,10) then (3,1) then
/// the first platform-0 record, and only formats 4 and 12 are parsed. Mappings
/// to glyph 0 are dropped. `subtable_formats` lists every record's format.
fn ruby_cmap(font: &FontRef) -> Option<(usize, String, Vec<u16>)> {
    let cmap = font.cmap().ok()?;
    let data = cmap.offset_data();
    let recs = cmap.encoding_records();
    let pick = |f: &dyn Fn(u16, u16) -> bool| {
        recs.iter()
            .position(|r| f(r.platform_id() as u16, r.encoding_id()))
    };
    let idx = pick(&|p, e| p == 3 && e == 10)
        .or_else(|| pick(&|p, e| p == 3 && e == 1))
        .or_else(|| pick(&|p, _| p == 0));
    let mut cps: Vec<u32> = Vec::new();
    if let Some(i) = idx {
        let sub = cmap.subtable(i as u16);
        if let Ok(sub) = sub {
            let format = recs[i]
                .subtable_offset()
                .non_null()
                .and_then(|o| data.read_at::<u16>(o).ok());
            if matches!(format, Some(4) | Some(12)) {
                cps = sub
                    .iter()
                    .filter(|(_, g)| g.to_u32() != 0)
                    .map(|(c, _)| c)
                    .collect();
            }
        }
    }
    cps.sort_unstable();
    cps.dedup();
    let mut h = Sha256::new();
    for c in &cps {
        h.update(c.to_be_bytes());
    }
    let digest = hex(&h.finalize());
    let mut formats: Vec<u16> = recs
        .iter()
        .filter_map(|r| {
            r.subtable_offset()
                .non_null()
                .and_then(|o| data.read_at::<u16>(o).ok())
        })
        .collect();
    formats.sort_unstable();
    formats.dedup();
    Some((cps.len(), digest, formats))
}

#[test]
fn cmap_matches_ruby_for_every_ci_font() {
    let Some(corpus) = corpus() else { return };
    let exp = expected();
    let names = fonts_with(&exp, "cmap");
    assert!(
        names.len() >= 20,
        "expected cmap entries, got {}",
        names.len()
    );
    let mut checked = 0;
    for name in names {
        let bytes = open(&corpus, &name);
        let font = FontRef::new(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let want = &exp["fonts"][&name]["cmap"];
        let got = match ruby_cmap(&font) {
            Some(g) => g,
            None => {
                // FINDING: a zero-length cmap table (quirk-empty-cmap-table) makes
                // read-fonts' `cmap()` fail, where Ruby yields an empty result. The
                // adapter must map "table present, zero bytes" to the empty answer.
                let len = font.table_data(Tag::new(b"cmap")).map(|d| d.len());
                assert_eq!(len, Some(0), "{name}: cmap() failed on a non-empty table");
                assert!(name.contains("empty-cmap"), "{name}");
                (0, sha_hex(&[]), Vec::new())
            }
        };
        assert_eq!(
            got.0 as u64,
            want["count"].as_u64().unwrap(),
            "{name}: count"
        );
        assert_eq!(
            got.1,
            want["sha256"].as_str().unwrap(),
            "{name}: codepoint digest"
        );
        let wf: Vec<u16> = want["subtable_formats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u16)
            .collect();
        assert_eq!(got.2, wf, "{name}: subtable formats");
        checked += 1;
    }
    eprintln!("cmap: {checked} fonts match Ruby");
}

/// skrifa's `Charmap` applies its own best-subtable rule and parses more
/// formats than Ruby, so parity is a coincidence of this corpus, not a
/// guarantee; the adapter reproduces Ruby's selection over read-fonts instead.
/// Here: on every corpus font the nonzero-gid mapping count agrees with Ruby.
#[test]
fn skrifa_charmap_count_agrees_on_corpus() {
    use skrifa::MetadataProvider;
    let Some(corpus) = corpus() else { return };
    let exp = expected();
    for name in fonts_with(&exp, "cmap") {
        let bytes = open(&corpus, &name);
        let font = skrifa::FontRef::new(&bytes).unwrap();
        let n = font
            .charmap()
            .mappings()
            .filter(|(_, g)| g.to_u32() != 0)
            .count() as u64;
        assert_eq!(
            n,
            exp["fonts"][&name]["cmap"]["count"].as_u64().unwrap(),
            "{name}"
        );
    }
}

// ---------------------------------------------------------------- glyf

fn glyf_context<'a>(
    font: &FontRef<'a>,
) -> (
    read_fonts::tables::glyf::Glyf<'a>,
    read_fonts::tables::loca::Loca<'a>,
    u32,
) {
    let head = font.head().unwrap();
    let n = font.maxp().unwrap().num_glyphs() as u32;
    (
        font.glyf().unwrap(),
        font.loca(head.index_to_loc_format() == 1).unwrap(),
        n,
    )
}

/// Ruby golden `outlines.digest`: SHA-256 over the 32-byte SHA-256 of each
/// glyph's byte range (`glyf.byteslice(offset, size) || ""`).
fn outline_digest(font: &FontRef) -> (u32, String) {
    let (_, loca, n) = glyf_context(font);
    let glyf = font.table_data(Tag::new(b"glyf")).unwrap();
    let raw = glyf.as_bytes();
    let mut all = Sha256::new();
    for gid in 0..n as usize {
        let (a, b) = (
            loca.get_raw(gid).unwrap() as usize,
            loca.get_raw(gid + 1).unwrap() as usize,
        );
        let bytes: &[u8] = if a <= raw.len() {
            &raw[a..b.min(raw.len())]
        } else {
            &[]
        };
        all.update(Sha256::digest(bytes));
    }
    (n, hex(&all.finalize()))
}

#[test]
fn glyf_raw_outline_digest_matches_ruby() {
    let Some(corpus) = corpus() else { return };
    let exp = expected();
    let mut checked = 0;
    for name in fonts_with(&exp, "glyf_parsed") {
        let bytes = open(&corpus, &name);
        let font = FontRef::new(&bytes).unwrap();
        let want = &exp["fonts"][&name]["outlines"];
        assert_eq!(want["kind"], "glyf", "{name}");
        let (n, digest) = outline_digest(&font);
        assert_eq!(
            n as u64,
            want["count"].as_u64().unwrap(),
            "{name}: glyph count"
        );
        assert_eq!(
            digest,
            want["digest"].as_str().unwrap(),
            "{name}: outline digest"
        );
        checked += 1;
    }
    eprintln!("glyf raw digest: {checked} fonts match Ruby");
}

fn le_hash(v: impl Iterator<Item = i32>) -> String {
    let mut h = Sha256::new();
    for x in v {
        h.update(x.to_le_bytes());
    }
    hex(&h.finalize())
}

/// The same per-glyph line the Ruby generator emits, built from read-fonts'
/// parsed glyphs (contours, points, bbox, point coordinates, on-curve bits,
/// composite component ids).
fn glyph_line(glyph: Option<Glyph>) -> String {
    match glyph {
        None => "e".into(),
        Some(Glyph::Simple(g)) => {
            let n = g.num_points();
            let mut pts = vec![Point::<i32>::default(); n];
            let mut fl = vec![PointFlags::default(); n];
            g.read_points_fast(&mut pts, &mut fl).unwrap();
            let ends: Vec<String> = g
                .end_pts_of_contours()
                .iter()
                .map(|e| e.get().to_string())
                .collect();
            let oc: Vec<u8> = fl.iter().map(|f| f.is_on_curve() as u8).collect();
            format!(
                "s,{},{},{},{},{},{},{},{},{},{}",
                g.number_of_contours(),
                n,
                g.x_min(),
                g.y_min(),
                g.x_max(),
                g.y_max(),
                ends.join(" "),
                le_hash(pts.iter().map(|p| p.x)),
                le_hash(pts.iter().map(|p| p.y)),
                sha_hex(&oc)
            )
        }
        Some(Glyph::Composite(g)) => {
            let ids: Vec<String> = g
                .components()
                .map(|c| c.glyph.to_u32().to_string())
                .collect();
            format!("c,{}", ids.join(" "))
        }
    }
}

#[test]
fn glyf_parsed_glyphs_match_ruby() {
    let Some(corpus) = corpus() else { return };
    let exp = expected();
    let mut checked = 0;
    for name in fonts_with(&exp, "glyf_parsed") {
        let bytes = open(&corpus, &name);
        let font = FontRef::new(&bytes).unwrap();
        let (glyf, loca, n) = glyf_context(&font);
        let lines: Vec<String> = (0..n)
            .map(|gid| {
                glyph_line(
                    loca.get_glyf(GlyphId::new(gid), &glyf)
                        .unwrap_or_else(|e| panic!("{name} gid {gid}: {e}")),
                )
            })
            .collect();
        let want = &exp["fonts"][&name]["glyf_parsed"];
        assert_eq!(
            lines.len() as u64,
            want["num_glyphs"].as_u64().unwrap(),
            "{name}"
        );
        for (kind, c) in want["kinds"].as_object().unwrap() {
            let got = lines
                .iter()
                .filter(|l| l.starts_with(kind.as_str()))
                .count() as u64;
            assert_eq!(got, c.as_u64().unwrap(), "{name}: kind {kind}");
        }
        assert_eq!(
            sha_hex(lines.join("\n").as_bytes()),
            want["sha256"].as_str().unwrap(),
            "{name}: parsed glyph digest"
        );
        checked += 1;
    }
    eprintln!("glyf parsed: {checked} fonts match Ruby");
}

// ---------------------------------------------------------------- gvar

fn tuple_str(t: &read_fonts::tables::variations::Tuple) -> String {
    (0..t.len())
        .map(|i| t.get(i).unwrap().to_bits().to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn gvar_matches_ruby_parse() {
    let Some(corpus) = corpus() else { return };
    let exp = expected();
    let names = fonts_with(&exp, "gvar");
    assert_eq!(names.len(), 4, "variable ttf fonts with gvar: {names:?}");
    for name in names {
        let bytes = open(&corpus, &name);
        let font = FontRef::new(&bytes).unwrap();
        let gvar = font.gvar().unwrap();
        let want = &exp["fonts"][&name]["gvar"];
        assert_eq!(
            [gvar.version().major as u64, gvar.version().minor as u64],
            [
                want["version"][0].as_u64().unwrap(),
                want["version"][1].as_u64().unwrap()
            ],
            "{name}: version"
        );
        assert_eq!(
            gvar.axis_count() as u64,
            want["axis_count"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            gvar.shared_tuple_count() as u64,
            want["shared_tuple_count"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            gvar.glyph_count() as u64,
            want["glyph_count"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            gvar.flags().bits() as u64,
            want["flags"].as_u64().unwrap(),
            "{name}: flags"
        );

        let shared = gvar.shared_tuples().unwrap();
        let shared_lines: Vec<String> = shared
            .tuples()
            .iter()
            .map(|t| tuple_str(&t.unwrap()))
            .collect();
        assert_eq!(
            sha_hex(shared_lines.join("\n").as_bytes()),
            want["shared_tuples_sha256"].as_str().unwrap(),
            "{name}: shared tuples"
        );

        let mut with_data = 0u64;
        let lines: Vec<String> = (0..gvar.glyph_count() as u32)
            .map(|gid| {
                let gid = GlyphId::new(gid);
                let Some(raw) = gvar.data_for_gid(gid).unwrap() else {
                    return "-".into();
                };
                with_data += 1;
                let data = gvar.glyph_variation_data(gid).unwrap().unwrap();
                let tuples: Vec<String> = data
                    .tuples()
                    .map(|t| {
                        let (s, e) = match (t.intermediate_start(), t.intermediate_end()) {
                            (Some(s), Some(e)) => (tuple_str(&s), tuple_str(&e)),
                            _ => (String::new(), String::new()),
                        };
                        format!("{}|{}|{}", tuple_str(&t.peak()), s, e)
                    })
                    .collect();
                // tuple count as stored in the header: low 12 bits of the first u16.
                let tc = u16::from_be_bytes([raw.as_bytes()[0], raw.as_bytes()[1]]) & 0x0FFF;
                format!(
                    "{}:{}:{}:{}",
                    sha_hex(raw.as_bytes()),
                    raw.len(),
                    tc,
                    tuples.join(";")
                )
            })
            .collect();
        assert_eq!(
            with_data,
            want["glyphs_with_data"].as_u64().unwrap(),
            "{name}: glyphs with data"
        );
        assert_eq!(
            sha_hex(lines.join("\n").as_bytes()),
            want["glyphs_sha256"].as_str().unwrap(),
            "{name}: per-glyph variation digest"
        );
    }
}

// ------------------------------------------------- skrifa outlines (extra)

/// Beyond the Ruby facade (no parsed-outline or instance golden exists yet):
/// skrifa draws every non-empty glyph at the default location and at the
/// maximum of every axis, and gvar actually moves points (coordinate sums differ).
#[test]
fn skrifa_draws_every_glyph_default_and_varied() {
    use skrifa::MetadataProvider;
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::DrawSettings;
    #[derive(Default)]
    struct Sum(f64);
    impl skrifa::outline::OutlinePen for Sum {
        fn move_to(&mut self, x: f32, y: f32) {
            self.0 += (x + y) as f64
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.0 += (x + y) as f64
        }
        fn quad_to(&mut self, a: f32, b: f32, x: f32, y: f32) {
            self.0 += (a + b + x + y) as f64
        }
        fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) {
            self.0 += (a + b + c + d + x + y) as f64
        }
        fn close(&mut self) {}
    }
    let Some(corpus) = corpus() else { return };
    let exp = expected();
    for name in fonts_with(&exp, "gvar") {
        let bytes = open(&corpus, &name);
        let font = skrifa::FontRef::new(&bytes).unwrap();
        let outlines = font.outline_glyphs();
        let axes = font.axes();
        let loc = axes.location(axes.iter().map(|a| (a.tag(), a.max_value())));
        let (mut drawn, mut moved) = (0u32, 0u32);
        for g in 0..font.maxp().unwrap().num_glyphs() as u32 {
            let Some(o) = outlines.get(GlyphId::new(g)) else {
                continue;
            };
            let (mut a, mut b) = (Sum::default(), Sum::default());
            o.draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                &mut a,
            )
            .unwrap();
            o.draw(DrawSettings::unhinted(Size::unscaled(), &loc), &mut b)
                .unwrap();
            drawn += 1;
            moved += (a.0 != b.0) as u32;
        }
        eprintln!(
            "{name}: skrifa drew {drawn} glyphs at default and at axis max; {moved} moved under gvar"
        );
        assert!(
            drawn > 700 && moved * 2 > drawn,
            "{name}: drawn {drawn}, moved {moved}"
        );
    }
}
