//! `ot_os2` (Ruby `Os2Check`): usWeightClass/usWidthClass ranges, reserved
//! fsSelection bits, typo/win metrics sign, all-zero PANOSE.

use crate::audit::ruby::need;
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_os2";
const FS_SELECTION_RESERVED: u16 = 0xFC00;

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("OS/2") {
        return Ok(Vec::new());
    }
    let os2 = need(font.os2(), "us_weight_class")?;
    let mut issues = Vec::new();
    let w = i64::from(os2.us_weight_class);
    if !(1..=1000).contains(&w) {
        issues.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!("OS/2.usWeightClass is {w} but must be between 1 and 1000"),
            )
            .at("os2.us_weight_class"),
        );
    }
    let w = i64::from(os2.us_width_class);
    if !(1..=9).contains(&w) {
        issues.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!("OS/2.usWidthClass is {w} but must be between 1 and 9"),
            )
            .at("os2.us_width_class"),
        );
    }
    let fs = os2.fs_selection;
    if fs & FS_SELECTION_RESERVED != 0 {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!("OS/2.fsSelection uses reserved bits (0x{fs:x}, bits 10-15 must be 0)"),
            )
            .at("os2.fs_selection"),
        );
    }
    if os2.s_typo_ascender <= 0 {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "OS/2.sTypoAscender is {} but should be positive",
                    os2.s_typo_ascender
                ),
            )
            .at("os2.s_typo_ascender"),
        );
    }
    if os2.s_typo_descender > 0 {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "OS/2.sTypoDescender is {} but should be \u{2264} 0",
                    os2.s_typo_descender
                ),
            )
            .at("os2.s_typo_descender"),
        );
    }
    if os2.us_win_ascent == 0 {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "OS/2.usWinAscent is {} but should be positive",
                    os2.us_win_ascent
                ),
            )
            .at("os2.us_win_ascent"),
        );
    }
    if os2.us_win_descent == 0 {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "OS/2.usWinDescent is {} but should be positive",
                    os2.us_win_descent
                ),
            )
            .at("os2.us_win_descent"),
        );
    }
    if os2.panose.iter().all(|&b| b == 0) {
        issues.push(
            Issue::new(
                CODE,
                Severity::Info,
                "OS/2.panose is all zeros \u{2014} PANOSE classification helps font matching in some applications",
            )
            .at("os2.panose"),
        );
    }
    Ok(issues)
}
