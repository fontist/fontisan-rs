//! `ot_cff` (Ruby `CffTableCheck`). CFF2 yields no issues: Ruby's
//! `font.table("CFF2")` is nil (measured on SourceSans3VF-Upright.otf). The
//! Name-INDEX-empty rule can never fire after a successful parse.
use crate::audit::ruby::{check_error, need};
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_cff";

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if font.has_table("CFF2") || !font.has_table("CFF ") {
        return Ok(vec![]);
    }
    let Some(count) = font.cff_charstring_count().map_err(check_error)? else {
        return Ok(vec![]);
    };
    if !font.has_table("maxp") {
        return Ok(vec![]);
    }
    let maxp = need(font.maxp(), "num_glyphs")?.num_glyphs as usize;
    if count == maxp {
        return Ok(vec![]);
    }
    Ok(vec![
        Issue::new(
            CODE,
            Severity::Error,
            format!("CFF  CharStrings count ({count}) does not match maxp.numGlyphs ({maxp})"),
        )
        .at("cff .charstrings"),
    ])
}
