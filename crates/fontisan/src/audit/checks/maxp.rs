//! `ot_maxp` (Ruby `MaxpCheck`).
use crate::audit::ruby::{hex, need};
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_maxp";

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("maxp") {
        return Ok(vec![]);
    }
    let maxp = need(font.maxp(), "version_raw")?;
    let mut v = Vec::new();
    let raw = i64::from(maxp.version_raw);
    let truetype = font.has_table("glyf");
    if truetype && raw != 0x0001_0000 {
        v.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!(
                    "maxp.version is 0x{} but must be 1.0 (0x10000) for TrueType fonts",
                    hex(raw)
                ),
            )
            .at("maxp.version"),
        );
    } else if !truetype && raw != 0x0000_5000 {
        v.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "maxp.version is 0x{} but should be 0.5 (0x5000) for CFF-based fonts",
                    hex(raw)
                ),
            )
            .at("maxp.version"),
        );
    }
    let n = maxp.num_glyphs;
    if n < 1 {
        v.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!("maxp.numGlyphs is {n} but must be between 1 and 65535"),
            )
            .at("maxp.num_glyphs"),
        );
    }
    if let Some(f) = maxp.v1 {
        let zones = f[4];
        if !(1..=2).contains(&zones) {
            v.push(
                Issue::new(
                    CODE,
                    Severity::Warning,
                    format!("maxp.maxZones is {zones} but must be 1 or 2 per the OpenType spec"),
                )
                .at("maxp.max_zones"),
            );
        }
        let stack = f[9];
        if stack > 1000 {
            v.push(
                Issue::new(
                    CODE,
                    Severity::Warning,
                    format!(
                        "maxp.maxStackElements is {stack} \u{2014} excessively large values waste interpreter memory"
                    ),
                )
                .at("maxp.max_stack_elements"),
            );
        }
    }
    Ok(v)
}
