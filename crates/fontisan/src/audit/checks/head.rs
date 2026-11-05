//! `ot_head` (Ruby `HeadCheck`).
use crate::audit::ruby::need;
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_head";
const MAGIC: u32 = 0x5F0F_3CF5;

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("head") {
        return Ok(vec![]);
    }
    let head = need(font.head(), "magic_number")?;
    let mut v = Vec::new();
    if head.magic_number != MAGIC {
        v.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!(
                    "head.magicNumber is 0x{:x} but must be 0x{MAGIC:x}",
                    head.magic_number
                ),
            )
            .at("head.magic_number"),
        );
    }
    let upm = head.units_per_em;
    if !(16..=16_384).contains(&upm) {
        v.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!("head.unitsPerEm is {upm} but must be between 16 and 16384"),
            )
            .at("head.units_per_em"),
        );
    }
    let fmt = head.index_to_loc_format;
    if fmt != 0 && fmt != 1 {
        v.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!("head.indexToLocFormat is {fmt} but must be 0 (short) or 1 (long)"),
            )
            .at("head.index_to_loc_format"),
        );
    }
    if head.created_raw == 0 {
        v.push(
            Issue::new(
                CODE,
                Severity::Info,
                "head.created is 0 \u{2014} creation date not set",
            )
            .at("head.created"),
        );
    }
    if head.modified_raw == 0 {
        v.push(
            Issue::new(
                CODE,
                Severity::Info,
                "head.modified is 0 \u{2014} modification date not set",
            )
            .at("head.modified"),
        );
    }
    if head.mac_style & 0xFF00 != 0 {
        v.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "head.macStyle uses reserved bits (0x{:x}, bits 8-15 must be 0)",
                    head.mac_style
                ),
            )
            .at("head.mac_style"),
        );
    }
    Ok(v)
}
