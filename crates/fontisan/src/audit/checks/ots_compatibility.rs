//! `ots_compatibility` (Ruby `OtsCompatibilityCheck`): approximations of what the
//! OpenType Sanitiser rejects (head magic/unitsPerEm, post version, maxp glyph
//! count, English family/PostScript names, Unicode cmap, OS/2 classes, outlines).
//! Any table that fails to parse raises, aborting the whole check (a fatal issue).

use super::rubyfmt::{fixed, required, ruby_float};
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ots_compatibility";
const MAX_GLYPHS: u32 = 0xFFFD;
const VALID_POST_VERSIONS: [f64; 5] = [1.0, 2.0, 2.5, 3.0, 4.0];
const HEAD_MAGIC: u32 = 0x5F0F_3CF5;
const MIN_UPM: u32 = 16;
const MAX_UPM: u32 = 16_384;

fn missing(tag: &str) -> Issue {
    Issue::new(
        CODE,
        Severity::Error,
        format!("OTS: required table '{tag}' is missing"),
    )
    .at(format!("tables.{tag}"))
}

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    let mut issues = Vec::new();
    check_head(font, &mut issues)?;
    check_post(font, &mut issues)?;
    check_maxp(font, &mut issues)?;
    check_name(font, &mut issues)?;
    check_cmap(font, &mut issues)?;
    check_os2(font, &mut issues)?;
    check_outlines(font, &mut issues);
    Ok(issues)
}

fn check_head(font: &Font<'_>, out: &mut Vec<Issue>) -> Result<(), CheckError> {
    if !font.has_table("head") {
        out.push(missing("head"));
        return Ok(());
    }
    let head = required(font.head(), "magic_number")?;
    if head.magic_number != HEAD_MAGIC {
        out.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!(
                    "OTS: head.magicNumber is 0x{:x} but must be 0x{HEAD_MAGIC:x}",
                    head.magic_number
                ),
            )
            .at("head.magic_number"),
        );
    }
    let upm = u32::from(head.units_per_em);
    if !(MIN_UPM..=MAX_UPM).contains(&upm) {
        out.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!(
                    "OTS: head.unitsPerEm is {upm} but must be between {MIN_UPM} and {MAX_UPM}"
                ),
            )
            .at("head.units_per_em"),
        );
    }
    Ok(())
}

fn check_post(font: &Font<'_>, out: &mut Vec<Issue>) -> Result<(), CheckError> {
    if !font.has_table("post") {
        out.push(missing("post"));
        return Ok(());
    }
    let post = required(font.post(), "version")?;
    let version = fixed(post.version_raw);
    if !VALID_POST_VERSIONS.contains(&version) {
        out.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!(
                    "OTS: post.version is {} but must be one of 1.0, 2.0, 2.5, 3.0, 4.0",
                    ruby_float(version)
                ),
            )
            .at("post.version"),
        );
    }
    Ok(())
}

fn check_maxp(font: &Font<'_>, out: &mut Vec<Issue>) -> Result<(), CheckError> {
    if !font.has_table("maxp") {
        out.push(missing("maxp"));
        return Ok(());
    }
    let n = u32::from(required(font.maxp(), "num_glyphs")?.num_glyphs);
    if n > MAX_GLYPHS {
        out.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!("OTS: maxp.numGlyphs is {n} but OTS rejects fonts with more than {MAX_GLYPHS} glyphs"),
            )
            .at("maxp.num_glyphs"),
        );
    }
    Ok(())
}

fn check_name(font: &Font<'_>, out: &mut Vec<Issue>) -> Result<(), CheckError> {
    if !font.has_table("name") {
        out.push(missing("name"));
        return Ok(());
    }
    let name = required(font.name(), "english_name")?;
    if name.english_name(1).unwrap_or_default().is_empty() {
        out.push(
            Issue::new(
                CODE,
                Severity::Error,
                "OTS: name table has no English Family name (nameID 1)",
            )
            .at("name.nameID.1"),
        );
    }
    if name.english_name(6).unwrap_or_default().is_empty() {
        out.push(
            Issue::new(
                CODE,
                Severity::Error,
                "OTS: name table has no English PostScript name (nameID 6)",
            )
            .at("name.nameID.6"),
        );
    }
    Ok(())
}

fn check_cmap(font: &Font<'_>, out: &mut Vec<Issue>) -> Result<(), CheckError> {
    if !font.has_table("cmap") {
        out.push(missing("cmap"));
        return Ok(());
    }
    let cmap = required(font.cmap(), "unicode_mappings")?;
    if cmap.codepoints().is_empty() {
        out.push(
            Issue::new(
                CODE,
                Severity::Error,
                "OTS: cmap has no Unicode subtable or no codepoint mappings",
            )
            .at("cmap.unicode_mappings"),
        );
    }
    Ok(())
}

fn check_os2(font: &Font<'_>, out: &mut Vec<Issue>) -> Result<(), CheckError> {
    if !font.has_table("OS/2") {
        out.push(missing("OS/2"));
        return Ok(());
    }
    let os2 = required(font.os2(), "us_weight_class")?;
    let w = os2.us_weight_class;
    if !(1..=1000).contains(&w) {
        out.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!("OTS: OS/2.usWeightClass is {w} but must be between 1 and 1000"),
            )
            .at("os2.us_weight_class"),
        );
    }
    let w = os2.us_width_class;
    if !(1..=9).contains(&w) {
        out.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!("OTS: OS/2.usWidthClass is {w} but must be between 1 and 9"),
            )
            .at("os2.us_width_class"),
        );
    }
    Ok(())
}

fn check_outlines(font: &Font<'_>, out: &mut Vec<Issue>) {
    if font.has_table("glyf") || font.has_table("CFF ") || font.has_table("CFF2") {
        return;
    }
    out.push(
        Issue::new(
            CODE,
            Severity::Error,
            "OTS: font has neither 'glyf' nor 'CFF '/'CFF2' \u{2014} no outline data",
        )
        .at("outline_tables"),
    );
}
