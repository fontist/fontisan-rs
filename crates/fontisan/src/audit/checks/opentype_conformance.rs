//! `opentype_conformance` (Ruby `OpenTypeConformanceCheck`): required tables for
//! the sfnt flavor, loca size against head/maxp, hhea.numberOfHMetrics bounds.
//! WOFF/WOFF2 faces raise in Ruby (`WoffFont` has no `tables`); that path is
//! pending until those containers load (card 1e-wire).

use super::rubyfmt::{nil_method, required};
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "opentype_conformance";

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    let mut issues = Vec::new();

    // validate_required_tables
    let mut required_tags = vec![
        "head", "hhea", "maxp", "hmtx", "name", "post", "cmap", "OS/2",
    ];
    if font.has_table("glyf") {
        required_tags.extend(["glyf", "loca"]);
    } else if font.has_table("CFF2") {
        required_tags.push("CFF2");
    } else if font.has_table("CFF ") {
        required_tags.push("CFF ");
    }
    for tag in required_tags {
        if !font.has_table(tag) {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Error,
                    format!("Required table '{tag}' is missing for this sfnt flavor"),
                )
                .at(format!("tables.{tag}")),
            );
        }
    }

    // validate_loca_format
    if font.has_table("head") && font.has_table("loca") {
        let head = required(font.head(), "index_to_loc_format")?;
        let loca_entry = font.entries().iter().find(|e| &e.tag == b"loca");
        // `loca` is nil when its data starts past EOF
        if let (Some(entry), Some(_)) = (loca_entry, font.table_data("loca")) {
            let format = head.index_to_loc_format;
            // `maxp&.num_glyphs.to_i`: a missing maxp counts as 0 glyphs
            let num_glyphs = match font.maxp().map_err(super::rubyfmt::sfnt_err)? {
                Some(m) => u64::from(m.num_glyphs),
                None => 0,
            };
            if num_glyphs != 0 {
                let expected = if format == 0 {
                    (num_glyphs + 1) * 2
                } else {
                    (num_glyphs + 1) * 4
                };
                let actual = u64::from(entry.length);
                if actual != expected {
                    issues.push(
                        Issue::new(
                            CODE,
                            Severity::Warning,
                            format!(
                                "loca table size ({actual} bytes) doesn't match head.indexToLocFormat={format} + numGlyphs={num_glyphs} (expected {expected} bytes)"
                            ),
                        )
                        .at("head.index_to_loc_format"),
                    );
                }
            }
        }
    }

    // validate_hhea_metrics
    if font.has_table("hhea") && font.has_table("maxp") {
        let hhea = font
            .hhea()
            .map_err(super::rubyfmt::sfnt_err)?
            .ok_or_else(|| nil_method("number_of_h_metrics"))?;
        let maxp = font
            .maxp()
            .map_err(super::rubyfmt::sfnt_err)?
            .ok_or_else(|| nil_method("num_glyphs"))?;
        let (metrics, glyphs) = (hhea.number_of_h_metrics, maxp.num_glyphs);
        if !(metrics > 0 && metrics <= glyphs) {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Error,
                    format!(
                        "hhea.numberOfHMetrics ({metrics}) must be between 1 and maxp.numGlyphs ({glyphs})"
                    ),
                )
                .at("hhea.number_of_h_metrics"),
            );
        }
    }
    Ok(issues)
}
