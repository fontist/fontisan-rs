//! `ot_kern` (Ruby `KernCheck`).
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_kern";

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("kern") {
        return Ok(vec![]);
    }
    let mut v = vec![
        Issue::new(
            CODE,
            Severity::Warning,
            "kern table is present but deprecated by the OpenType spec \u{2014} use GPOS instead. Some modern renderers (HarfBuzz, Core Text) ignore kern",
        )
        .at("tables.kern"),
    ];
    if !font.has_table("GPOS") {
        v.push(
            Issue::new(
                CODE,
                Severity::Warning,
                "kern table present but no GPOS table \u{2014} kerning data should be migrated to a GPOS 'kern' feature for full renderer support",
            )
            .at("tables.GPOS"),
        );
    }
    Ok(v)
}
