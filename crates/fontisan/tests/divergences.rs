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

/// A copy of `bytes` (a single-face sfnt) without its `cmap` table. Tables keep
/// their order and are 4-byte aligned, so the result is deterministic.
fn drop_cmap(bytes: &[u8]) -> Vec<u8> {
    let u32_at = |o: usize| u32::from_be_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
    let n = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    let kept: Vec<usize> = (0..n)
        .map(|i| 12 + 16 * i)
        .filter(|&r| &bytes[r..r + 4] != b"cmap")
        .collect();
    let m = kept.len();
    let es = m.ilog2() as usize;
    let sr = 16 * (1usize << es);
    let mut out = bytes[..4].to_vec();
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
