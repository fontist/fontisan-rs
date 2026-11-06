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
