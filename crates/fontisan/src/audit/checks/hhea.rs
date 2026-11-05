//! `ot_hhea` (Ruby `HheaCheck`).
use crate::audit::ruby::need;
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_hhea";

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("hhea") {
        return Ok(vec![]);
    }
    let h = need(font.hhea(), "ascent")?;
    let mut v = Vec::new();
    if h.ascent <= 0 {
        v.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "hhea.ascent is {} but should be positive (ascender typically rises above the baseline)",
                    h.ascent
                ),
            )
            .at("hhea.ascent"),
        );
    }
    if h.descent > 0 {
        v.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "hhea.descent is {} but should be \u{2264} 0 (descender typically falls below the baseline)",
                    h.descent
                ),
            )
            .at("hhea.descent"),
        );
    }
    if h.line_gap < 0 {
        v.push(
            Issue::new(
                CODE,
                Severity::Info,
                format!(
                    "hhea.lineGap is {} \u{2014} negative line gap is unusual but not spec-prohibited",
                    h.line_gap
                ),
            )
            .at("hhea.line_gap"),
        );
    }
    if h.advance_width_max == 0 {
        v.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "hhea.advanceWidthMax is {} but should be positive (the widest glyph advance)",
                    h.advance_width_max
                ),
            )
            .at("hhea.advance_width_max"),
        );
    }
    Ok(v)
}
