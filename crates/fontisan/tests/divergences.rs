//! Cases the Phase 0 goldens do not cover, pinned to Ruby's measured output
//! (see docs/divergences.md). Skips when the corpus is absent;
//! `FONTISAN_SPIKE_REQUIRE_CORPUS=1` turns a skip into a failure.
mod common;

use common::*;
use fontisan::audit;
use fontisan::read;
use fontisan::sfnt::{LoadingMode, Options};

/// Measured in fontisan-phase0 (Phase 0 head):
///   bundle exec ruby -Ilib -e 'require "fontisan"; require "json";
///     %i[full metadata].each { |m| puts Fontisan::Reference.read(
///       "corpus/fonts/source-sans-3-regular-otf-woff/SourceSans3-Regular.otf.woff",
///       mode: m)[0]["mode"] }'
/// printed `full` then `metadata`: Ruby echoes the requested mode for WOFF1.
#[test]
fn woff1_read_echoes_the_requested_mode() {
    let Some(corpus) = corpus() else { return };
    let bytes = std::fs::read(corpus.join("fonts/SourceSans3-Regular.otf.woff")).unwrap();
    for (mode, want) in [
        (LoadingMode::Full, "full"),
        (LoadingMode::Metadata, "metadata"),
    ] {
        for lazy in [true, false] {
            let rows = read::read(&bytes, Options { mode, lazy }).unwrap();
            let json = rows[0].render();
            assert!(
                json.contains(&format!("\"mode\":\"{want}\"")),
                "mode={want} lazy={lazy}: {}",
                &json[..json.len().min(300)]
            );
        }
    }
}

/// Measured in fontisan-phase0:
///   bundle exec ruby -Ilib -e 'require "fontisan"; require "json";
///     puts JSON.generate(Fontisan::Reference.audit(
///       "corpus/derived/malformed-bad-checksum.ttf", validate: true)[0]
///       .slice("validation_issues","validation_summary"))'
/// The table_directory check dies on `entry.checksum.to_s(16)` (a BinData
/// Uint32be#to_s takes no argument) when the first non-head table's checksum
/// mismatches, so the report carries a fatal issue, not the mismatch error.
#[test]
fn bad_checksum_audit_reports_the_ruby_fatal() {
    let Some(corpus) = corpus() else { return };
    let bytes = std::fs::read(corpus.join("derived/malformed-bad-checksum.ttf")).unwrap();
    let reports = audit::audit(&bytes, "malformed-bad-checksum.ttf", true).unwrap();
    let json = reports[0].to_json().render();
    let fatal = r#"{"category":"table_directory","message":"Check table_directory failed to execute: ArgumentError: wrong number of arguments (given 1, expected 0)","severity":"fatal"}"#;
    assert!(json.contains(fatal), "{json}");
    let summary = r#""validation_summary":{"error_count":0,"fatal_count":1,"info_count":1,"passed":false,"total":3,"warning_count":1}"#;
    assert!(json.contains(summary), "{json}");
    assert!(!json.contains("checksum mismatch"), "{json}");
}

/// A copy of `bytes` (a single-face sfnt) without its `cmap` table.
fn drop_cmap(bytes: &[u8]) -> Vec<u8> {
    drop_table(bytes, b"cmap")
}

/// A copy of `bytes` (a single-face sfnt) without table `tag`. Tables keep
/// their order and are 4-byte aligned, so the result is deterministic.
fn drop_table(bytes: &[u8], tag: &[u8; 4]) -> Vec<u8> {
    drop_table_at(bytes, 0, tag)
}

/// `drop_table` for the sfnt whose directory starts at `head` (a collection
/// member); table offsets are absolute in `bytes`, the result is standalone.
fn drop_table_at(bytes: &[u8], head: usize, tag: &[u8; 4]) -> Vec<u8> {
    let u32_at = |o: usize| u32::from_be_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
    let n = usize::from(u16::from_be_bytes([bytes[head + 4], bytes[head + 5]]));
    let kept: Vec<usize> = (0..n)
        .map(|i| head + 12 + 16 * i)
        .filter(|&r| &bytes[r..r + 4] != tag)
        .collect();
    let m = kept.len();
    let es = m.ilog2() as usize;
    let sr = 16 * (1usize << es);
    let mut out = bytes[head..head + 4].to_vec();
    for v in [m, sr, es, m * 16 - sr] {
        out.extend_from_slice(&(v as u16).to_be_bytes());
    }
    let mut body: Vec<u8> = Vec::new();
    for &r in &kept {
        body.resize(body.len().next_multiple_of(4), 0);
        out.extend_from_slice(&bytes[r..r + 4]);
        out.extend_from_slice(&bytes[r + 4..r + 8]);
        out.extend_from_slice(&((12 + 16 * m + body.len()) as u32).to_be_bytes());
        out.extend_from_slice(&bytes[r + 12..r + 16]);
        body.extend_from_slice(&bytes[u32_at(r + 8)..u32_at(r + 8) + u32_at(r + 12)]);
    }
    out.extend(body);
    out
}

/// A collection with `tag` removed from every member, like the sweep's TTC drops.
fn drop_table_from_collection(bytes: &[u8], tag: &[u8; 4]) -> Vec<u8> {
    let count = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let members: Vec<Vec<u8>> = (0..count)
        .map(|i| {
            let off = u32::from_be_bytes(bytes[12 + 4 * i..16 + 4 * i].try_into().unwrap());
            drop_table_at(bytes, off as usize, tag)
        })
        .collect();
    collection_of(&members)
}

/// A TTC whose members are the given standalone sfnts.
fn collection_of(members: &[Vec<u8>]) -> Vec<u8> {
    let count = members.len();
    let mut out = b"ttcf".to_vec();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&(count as u32).to_be_bytes());
    let mut base = 12 + 4 * count;
    let mut body = Vec::new();
    for m in members {
        out.extend_from_slice(&(base as u32).to_be_bytes());
        let n = usize::from(u16::from_be_bytes([m[4], m[5]]));
        let mut m = m.clone();
        for t in 0..n {
            let o = 12 + 16 * t + 8;
            let v = u32::from_be_bytes(m[o..o + 4].try_into().unwrap()) + base as u32;
            m[o..o + 4].copy_from_slice(&v.to_be_bytes());
        }
        m.resize(m.len().next_multiple_of(4), 0);
        base += m.len();
        body.extend(m);
    }
    out.extend(body);
    out
}

/// Measured in fontisan-phase2 (Ruby 3.4.8), `TestOTF.otf` with `cmap` dropped
/// (this test's `drop_cmap`), for validate false and true:
///   bundle exec ruby -Ilib -rjson -rfontisan -e 'require "fontisan/reference";
///     puts JSON.generate(Fontisan::Reference.audit("nocmap.otf", validate: V))'
/// Ruby omits `total_codepoints` (nil) and `cmap_subtables` for a cmap-less
/// font; the other fields are unaffected. Pinned verbatim below.
#[test]
fn cmap_less_font_audit_omits_total_codepoints_like_ruby() {
    let Some(corpus) = corpus() else { return };
    let bytes = drop_cmap(&std::fs::read(corpus.join("fonts/TestOTF.otf")).unwrap());
    let ruby = [
        (
            false,
            r#"[{"axes":[],"bold":false,"family_name":"Test OTF","font_revision_raw":65536,"full_name":"Test OTF","is_variable":false,"italic":false,"num_fonts_in_source":1,"panose":"2 11 6 4 4 2 7 8 1 4","postscript_name":"TestOTF-Regular","source_file":"nocmap.otf","source_format":"otf","source_sha256":"81cbff3eb250b0a201a6c76df49639397529a50b6c93925fb782216e8d8c7d5d","subfamily_name":"Regular","total_glyphs":6,"version":"Version 1.000","weight_class":400,"width_class":5}]"#,
        ),
        (
            true,
            r#"[{"axes":[],"bold":false,"family_name":"Test OTF","font_revision_raw":65536,"full_name":"Test OTF","is_variable":false,"italic":false,"num_fonts_in_source":1,"panose":"2 11 6 4 4 2 7 8 1 4","postscript_name":"TestOTF-Regular","source_file":"nocmap.otf","source_format":"otf","source_sha256":"81cbff3eb250b0a201a6c76df49639397529a50b6c93925fb782216e8d8c7d5d","subfamily_name":"Regular","total_glyphs":6,"validation_issues":[{"category":"ots_compatibility","location":"tables.cmap","message":"OTS: required table 'cmap' is missing","severity":"error"},{"category":"hinting","location":"cff.private_dict","message":"CFF Private DICT has no BlueValues or StdHW/StdVW — alignment zones and stem widths are the primary hinting mechanism for CFF outlines","severity":"info"},{"category":"opentype_conformance","location":"tables.cmap","message":"Required table 'cmap' is missing for this sfnt flavor","severity":"error"},{"category":"naming_consistency","location":"name.consistency.full_name","message":"Full name 'Test OTF' doesn't match Family + Subfamily 'Test OTF Regular'","severity":"info"}],"validation_summary":{"error_count":2,"fatal_count":0,"info_count":2,"passed":false,"total":4,"warning_count":0},"version":"Version 1.000","weight_class":400,"width_class":5}]"#,
        ),
    ];
    for (validate, want) in ruby {
        let got = audit::audit(&bytes, "nocmap.otf", validate).unwrap();
        let got: serde_json::Value = serde_json::from_str(&got[0].to_json().render()).unwrap();
        let want: serde_json::Value = serde_json::from_str(want).unwrap();
        assert_eq!(got, want[0], "validate={validate}");
    }
}

/// Measured in fontisan-phase2 (Ruby 3.4.8), `TestOTF.otf` with `maxp` dropped
/// (this test's `drop_table`), same command as the cmap-less case. Ruby reports
/// `total_glyphs` 0 (not nil) without `maxp`. With validation, the
/// `format_round_trip` check is silent because Ruby's OTF->TTF conversion
/// raises "Font missing required maxp table" (`OutlineConverter#validate_source_tables`),
/// so no "glyph count changed from 0 to 6" warning appears.
#[test]
fn maxp_less_font_audit_matches_ruby() {
    let Some(corpus) = corpus() else { return };
    let bytes = drop_table(
        &std::fs::read(corpus.join("fonts/TestOTF.otf")).unwrap(),
        b"maxp",
    );
    let ruby = [
        (
            false,
            r#"[{"axes":[],"bold":false,"cmap_subtables":[4,6],"family_name":"Test OTF","font_revision_raw":65536,"full_name":"Test OTF","is_variable":false,"italic":false,"num_fonts_in_source":1,"panose":"2 11 6 4 4 2 7 8 1 4","postscript_name":"TestOTF-Regular","source_file":"nomaxp-TestOTF.otf","source_format":"otf","source_sha256":"1906eda710f9eb13ee768a97792147521b6de6371f9ebfedc7999c466e39e7b8","subfamily_name":"Regular","total_codepoints":5,"total_glyphs":0,"version":"Version 1.000","weight_class":400,"width_class":5}]"#,
        ),
        (
            true,
            r#"[{"axes":[],"bold":false,"cmap_subtables":[4,6],"family_name":"Test OTF","font_revision_raw":65536,"full_name":"Test OTF","is_variable":false,"italic":false,"num_fonts_in_source":1,"panose":"2 11 6 4 4 2 7 8 1 4","postscript_name":"TestOTF-Regular","source_file":"nomaxp-TestOTF.otf","source_format":"otf","source_sha256":"1906eda710f9eb13ee768a97792147521b6de6371f9ebfedc7999c466e39e7b8","subfamily_name":"Regular","total_codepoints":5,"total_glyphs":0,"validation_issues":[{"category":"ots_compatibility","location":"tables.maxp","message":"OTS: required table 'maxp' is missing","severity":"error"},{"category":"hinting","location":"cff.private_dict","message":"CFF Private DICT has no BlueValues or StdHW/StdVW — alignment zones and stem widths are the primary hinting mechanism for CFF outlines","severity":"info"},{"category":"opentype_conformance","location":"tables.maxp","message":"Required table 'maxp' is missing for this sfnt flavor","severity":"error"},{"category":"naming_consistency","location":"name.consistency.full_name","message":"Full name 'Test OTF' doesn't match Family + Subfamily 'Test OTF Regular'","severity":"info"}],"validation_summary":{"error_count":2,"fatal_count":0,"info_count":2,"passed":false,"total":4,"warning_count":0},"version":"Version 1.000","weight_class":400,"width_class":5}]"#,
        ),
    ];
    for (validate, want) in ruby {
        let got = audit::audit(&bytes, "nomaxp-TestOTF.otf", validate).unwrap();
        let got: serde_json::Value = serde_json::from_str(&got[0].to_json().render()).unwrap();
        let want: serde_json::Value = serde_json::from_str(want).unwrap();
        assert_eq!(got, want[0], "validate={validate}");
    }
}

/// Measured in fontisan-phase2 (Ruby 3.4.8):
///   Fontisan::Reference.convert("no<tag>-TestOTF.otf", to: :ttf, output: ...)
/// raises `Fontisan::Error` "Conversion failed: Transformation failed: Font
/// missing required <tag> table" for `head`, `hhea` and `maxp` dropped.
#[test]
fn cff_to_ttf_without_head_hhea_or_maxp_fails_like_ruby() {
    let Some(corpus) = corpus() else { return };
    let src = std::fs::read(corpus.join("fonts/TestOTF.otf")).unwrap();
    for tag in [b"head", b"hhea", b"maxp"] {
        let name = String::from_utf8_lossy(tag).into_owned();
        let err =
            fontisan::convert::convert(&drop_table(&src, tag), fontisan::convert::Target::Ttf, 0)
                .expect_err(&name);
        assert_eq!(
            err.message,
            format!("Conversion failed: Transformation failed: Font missing required {name} table"),
        );
    }
}

/// Measured in fontisan-phase2 (Ruby 3.4.8), `Fontisan::Reference.convert(
/// "no<tag>-TestOTF.otf", to: :ttf, output: ...)`: each raises
/// `Fontisan::Error` with the message below (Ruby repeats a clause once per
/// failing validation check; pinned verbatim for a single missing table).
#[test]
fn cff_to_ttf_without_post_name_os2_or_cmap_fails_like_ruby() {
    let Some(corpus) = corpus() else { return };
    let src = std::fs::read(corpus.join("fonts/TestOTF.otf")).unwrap();
    let want: [(&[u8; 4], &str); 4] = [
        (
            b"post",
            "Output validation failed: Required table 'post' is missing",
        ),
        (
            b"name",
            "Output validation failed: Font structure validation failed for check 'required_tables', Required table 'name' is missing, Required table 'name' is missing, Required table 'name' is missing, Required table 'name' is missing",
        ),
        (
            b"OS/2",
            "Output validation failed: Required table 'OS/2' is missing, Required table 'OS/2' is missing, Required table 'OS/2' is missing, Required table 'OS/2' is missing, Required table 'OS/2' is missing",
        ),
        (
            b"cmap",
            "Output validation failed: Required table 'cmap' is missing, Required table 'cmap' is missing, Required table 'cmap' is missing, Exception during check execution: NoMethodError - undefined method 'valid_glyph_indices?' for nil",
        ),
    ];
    for (tag, msg) in want {
        let name = String::from_utf8_lossy(tag).into_owned();
        let err =
            fontisan::convert::convert(&drop_table(&src, tag), fontisan::convert::Target::Ttf, 0)
                .expect_err(&name);
        assert_eq!(
            err.message,
            format!("Conversion failed: Transformation failed: {msg}"),
            "{name}"
        );
    }
}

/// Measured in fontisan-phase2 (Ruby 3.4.8), `TestTTF.ttf` with `head`, `hhea`
/// or `maxp` dropped, converted `to: :otf`: "Conversion failed: Transformation
/// failed: Font missing required <tag> table".
#[test]
fn ttf_to_otf_without_head_hhea_or_maxp_fails_like_ruby() {
    let Some(corpus) = corpus() else { return };
    let src = std::fs::read(corpus.join("fonts/TestTTF.ttf")).unwrap();
    for tag in [b"head", b"hhea", b"maxp"] {
        let name = String::from_utf8_lossy(tag).into_owned();
        let err =
            fontisan::convert::convert(&drop_table(&src, tag), fontisan::convert::Target::Otf, 0)
                .expect_err(&name);
        assert_eq!(
            err.message,
            format!("Conversion failed: Transformation failed: Font missing required {name} table"),
        );
    }
}

/// Ruby's audit JSON (validate false and true) for a font with one table
/// dropped, measured in fontisan-phase2 (Ruby 3.4.8) with the same command as
/// the cmap-less case; the keys are `<font>/<TAG with / removed>/<validate>`.
/// Covers the round-trip check, which Ruby silences when its conversion raises.
#[test]
fn audit_of_fonts_missing_a_table_matches_ruby() {
    let Some(corpus) = corpus() else { return };
    let fx: serde_json::Value =
        serde_json::from_str(include_str!("divergences/audit_missing_tables.json")).unwrap();
    for (key, want) in fx.as_object().unwrap() {
        let parts: Vec<&str> = key.split('/').collect();
        let (font, tag, validate) = (parts[0], parts[1], parts[2] == "true");
        let tag4: [u8; 4] = match tag {
            "OS2" => *b"OS/2",
            t => t.as_bytes().try_into().unwrap(),
        };
        let bytes = drop_table(
            &std::fs::read(corpus.join("fonts").join(font)).unwrap(),
            &tag4,
        );
        let name = format!("no{tag}-{font}");
        let got = audit::audit(&bytes, &name, validate).unwrap();
        let got: serde_json::Value = serde_json::from_str(&got[0].to_json().render()).unwrap();
        assert_eq!(&got, want, "{key}");
    }
}

/// Measured in fontisan-phase2 (Ruby 3.4.8), `TestTTF.ttf` with one table
/// dropped, `Fontisan::Reference.convert(..., to: :otf, output: ...)`. The
/// `name` text is Ruby's own encoding error, pinned verbatim.
#[test]
fn ttf_to_otf_without_loca_name_post_os2_or_cmap_fails_like_ruby() {
    let Some(corpus) = corpus() else { return };
    let src = std::fs::read(corpus.join("fonts/TestTTF.ttf")).unwrap();
    let want: [(&[u8; 4], &str); 5] = [
        (b"loca", "TrueType font missing required glyf or loca table"),
        (b"name", "item 0 must have BINARY encoding, got: UTF-8"),
        (
            b"post",
            "Output validation failed: Required table 'post' is missing",
        ),
        (
            b"OS/2",
            "Output validation failed: Required table 'OS/2' is missing, Required table 'OS/2' is missing, Required table 'OS/2' is missing, Required table 'OS/2' is missing, Required table 'OS/2' is missing",
        ),
        (
            b"cmap",
            "Output validation failed: Required table 'cmap' is missing, Required table 'cmap' is missing, Required table 'cmap' is missing, Exception during check execution: NoMethodError - undefined method 'valid_glyph_indices?' for nil",
        ),
    ];
    for (tag, msg) in want {
        let name = String::from_utf8_lossy(tag).into_owned();
        let err =
            fontisan::convert::convert(&drop_table(&src, tag), fontisan::convert::Target::Otf, 0)
                .expect_err(&name);
        assert_eq!(
            err.message,
            format!("Conversion failed: Transformation failed: {msg}"),
            "{name}"
        );
    }
}

/// The corpus file behind a sweep case name (`scripts/corpus/drop_table_sweep.rb`).
fn sweep_source(corpus: &std::path::Path, font: &str) -> Vec<u8> {
    let (dir, file) = match font {
        "noto-sans-jp-subset-regular-otf" => ("fonts", "NotoSansJP-Regular.otf"),
        "fonttools-test-woff" => ("fonts", "TestWOFF.woff"),
        "fonttools-test-otf" => ("fonts", "TestOTF.otf"),
        "fonttools-test-ttc" => ("fonts", "TestTTC.ttc"),
        "fonttools-test-ttc-v2" => ("fonts", "TestTTCv2.ttc"),
        "source-sans-3-regular-otf-woff" => ("fonts", "SourceSans3-Regular.otf.woff"),
        "source-sans-3-regular-ttf-woff" => ("fonts", "SourceSans3-Regular.ttf.woff"),
        "source-sans-3-regular-otf" => ("fonts", "SourceSans3-Regular.otf"),
        "source-sans-3-vf-upright-ttf" => ("fonts", "SourceSans3VF-Upright.ttf"),
        "source-sans-3-vf-italic-ttf" => ("fonts", "SourceSans3VF-Italic.ttf"),
        "source-sans-3-vf-upright-cff2" => ("fonts", "SourceSans3VF-Upright.otf"),
        "noto-sans-vf-wdth-wght" => ("fonts", "NotoSans[wdth,wght].ttf"),
        "mona-sans-vf-all-axes" => ("fonts", "MonaSansVF[wdth,wght,opsz,ital].ttf"),
        other => {
            let ext = if other.starts_with("ttc-") {
                "ttc"
            } else {
                "ttf"
            };
            return std::fs::read(corpus.join("derived").join(format!("{other}.{ext}")))
                .unwrap_or_else(|e| panic!("{font}: {e}"));
        }
    };
    std::fs::read(corpus.join(dir).join(file)).unwrap_or_else(|e| panic!("{font}: {e}"))
}

/// Ruby's `Fontisan.convert` outcome for sources with one table dropped, as the
/// drop-table sweep measured it (`convert_missing_tables.json`; key
/// `<target>|<font>|<tag>`, value `{"ok":true}` or `{"error":"<message>"}`).
/// Covers: required tables on WOFF 1 and OTTO-on-glyf sources, outline-less
/// sources, a collection whose members lost their outlines, and variable fonts
/// that lost gvar or CFF2.
#[test]
fn convert_of_fonts_missing_a_table_matches_ruby() {
    use fontisan::convert::{Target, convert};
    let Some(corpus) = corpus() else { return };
    let fx: serde_json::Value =
        serde_json::from_str(include_str!("divergences/convert_missing_tables.json")).unwrap();
    for (key, want) in fx.as_object().unwrap() {
        let parts: Vec<&str> = key.split('|').collect();
        let (to, font, tag) = (parts[0], parts[1], parts[2]);
        let tag4: [u8; 4] = tag.as_bytes().try_into().unwrap();
        let src = sweep_source(&corpus, font);
        let input = if src.starts_with(b"ttcf") {
            drop_table_from_collection(&src, &tag4)
        } else if src.starts_with(b"wOFF") {
            let sfnt = fontisan::woff::decode_woff(&src).unwrap();
            convert(&drop_table(&sfnt, &tag4), Target::Woff, 0).unwrap()
        } else {
            drop_table(&src, &tag4)
        };
        let target = match to {
            "ttf" => Target::Ttf,
            "otf" => Target::Otf,
            "woff" => Target::Woff,
            "woff2" => Target::Woff2,
            "otc" => Target::Otc,
            other => panic!("{other}"),
        };
        let got = convert(&input, target, 0);
        match (want.get("error"), got) {
            (Some(msg), Err(e)) => assert_eq!(e.message, msg.as_str().unwrap(), "{key}"),
            (Some(msg), Ok(_)) => panic!("{key}: ruby fails with {msg}, native wrote a font"),
            (None, Ok(_)) => {}
            (None, Err(e)) => panic!("{key}: ruby converts, native fails: {}", e.message),
        }
    }
}

/// `(glyf offsets, indexToLocFormat)` of the sfnt whose directory starts at `head`.
fn loca_offsets(b: &[u8], head: usize) -> (Vec<usize>, u16) {
    let u32_at = |o: usize| u32::from_be_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let n = usize::from(u16::from_be_bytes([b[head + 4], b[head + 5]]));
    let rec = |tag: &[u8; 4]| {
        (0..n)
            .map(|i| head + 12 + 16 * i)
            .find(|&r| &b[r..r + 4] == tag)
            .map(|r| (u32_at(r + 8), u32_at(r + 12)))
            .unwrap()
    };
    let (ho, _) = rec(b"head");
    let fmt = u16::from_be_bytes([b[ho + 50], b[ho + 51]]);
    let (mo, _) = rec(b"maxp");
    let ng = usize::from(u16::from_be_bytes([b[mo + 4], b[mo + 5]]));
    let (lo, _) = rec(b"loca");
    let offs = (0..=ng)
        .map(|i| {
            if fmt == 0 {
                2 * usize::from(u16::from_be_bytes([b[lo + 2 * i], b[lo + 2 * i + 1]]))
            } else {
                u32_at(lo + 4 * i)
            }
        })
        .collect();
    (offs, fmt)
}

/// Ruby 9716f23 (single font) and 7ac79b4 (collection): a short-loca glyf whose
/// 4-byte padded size passes 0x1FFFE is written with a long loca. Measured with
/// source-sans-3-vf-upright-ttf minus fvar and gvar: Ruby converts it to woff2,
/// and as both members of a TTC. The pinned `woff2_decompress`
/// (`FONTISAN_WOFF2_DECOMPRESS`) must read back a monotonic long loca.
#[test]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
fn woff2_widens_short_loca_when_padded_glyf_overflows() {
    use fontisan::convert::{Target, convert};
    let Some(corpus) = corpus() else { return };
    let src = std::fs::read(corpus.join("fonts/SourceSans3VF-Upright.ttf")).unwrap();
    let base = drop_table(&drop_table(&src, b"fvar"), b"gvar");
    let single = convert(&base, Target::Woff2, 0).expect("single font converts");
    let ttc = collection_of(&[base.clone(), base]);
    let coll = convert(&ttc, Target::Woff2, 0).expect("collection converts");
    let Some(bin) = std::env::var_os("FONTISAN_WOFF2_DECOMPRESS")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file())
    else {
        eprintln!("SKIPPED decode check: FONTISAN_WOFF2_DECOMPRESS unset");
        return;
    };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("woff2-loca");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, woff2) in [("single", single), ("collection", coll)] {
        let f = dir.join(format!("{name}.woff2"));
        std::fs::write(&f, &woff2).unwrap();
        assert!(
            std::process::Command::new(&bin)
                .arg(&f)
                .status()
                .unwrap()
                .success()
        );
        let out = std::fs::read(f.with_extension("ttf")).unwrap();
        let heads: Vec<usize> = if out.starts_with(b"ttcf") {
            let n = u32::from_be_bytes(out[8..12].try_into().unwrap()) as usize;
            (0..n)
                .map(|i| {
                    u32::from_be_bytes(out[12 + 4 * i..16 + 4 * i].try_into().unwrap()) as usize
                })
                .collect()
        } else {
            vec![0]
        };
        assert_eq!(heads.len(), if name == "single" { 1 } else { 2 });
        for h in heads {
            let (offs, fmt) = loca_offsets(&out, h);
            assert_eq!(fmt, 1, "{name}: indexToLocFormat");
            assert!(
                offs.windows(2).all(|w| w[0] <= w[1]),
                "{name}: loca not monotonic"
            );
            assert!(
                *offs.last().unwrap() > 0x1FFFE,
                "{name}: glyf is below the short-loca limit"
            );
        }
    }
}
