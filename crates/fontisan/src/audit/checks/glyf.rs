//! `ot_glyf` (Ruby `GlyfTableCheck`).
//!
//! The Ruby check walks every glyph through `glyf.glyph_for(gid, loca, head)`
//! and rescues any error to `nil`. In the audit path `font.table("loca")` is
//! never parsed with context (nothing calls `Loca#parse_with_context`), so
//! `glyph_for` raises `ArgumentError "loca table must be parsed ..."` for every
//! glyph and the contour/component rules (`endPoints are not monotonically
//! increasing`, `out-of-bounds GID`) are unreachable. Measured on every corpus
//! font: zero `ot_glyf` issues apart from the `_swallow/maxp-truncated` fatal.
//! What is reachable:
//!  * `font.table("maxp")` failing raises (a fatal issue from the runner);
//!  * `font.table("head")` failing inside the per-glyph block (outside the inner
//!    rescue) yields one warning per glyph.

use crate::audit::ruby::check_error;
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_glyf";

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !(font.has_table("glyf") && font.has_table("loca")) {
        return Ok(Vec::new());
    }
    // `glyf = font.table("glyf")` / `font.table_data["glyf"]` are nil past EOF.
    if font.table_data("glyf").is_none() {
        return Ok(Vec::new());
    }
    let num_glyphs = match font.maxp().map_err(check_error)? {
        Some(m) => m.num_glyphs,
        None => 0,
    };
    if num_glyphs == 0 {
        return Ok(Vec::new());
    }
    let mut issues = Vec::new();
    for gid in 0..num_glyphs {
        // `font.table("head")` re-raises on every glyph; glyph_for never yields a glyph.
        if font.head().is_err() {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Warning,
                    format!("glyf: failed to validate glyph at GID {gid}"),
                )
                .at(format!("glyf.glyph.{gid}")),
            );
        }
    }
    Ok(issues)
}
