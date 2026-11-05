//! `variable_font` (Ruby `VariableFontCheck`): fvar axis uniqueness and ranges,
//! instance coordinate ranges, and the gvar/HVAR/STAT companions.

use crate::audit::ruby::{check_error, fixed, float_to_s};
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "variable_font";

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("fvar") {
        return Ok(Vec::new());
    }
    let Some(fvar) = font.fvar().map_err(check_error)? else {
        // table data past EOF: `font.table("fvar")` is nil
        return Err(crate::audit::ruby::nil_method("axes"));
    };
    let mut issues = Vec::new();
    let axes = &fvar.axes;

    // check_axis_uniqueness: tally in first-occurrence order
    let mut tally: Vec<(&str, usize)> = Vec::new();
    for a in axes {
        match tally.iter_mut().find(|(t, _)| *t == a.tag) {
            Some((_, c)) => *c += 1,
            None => tally.push((&a.tag, 1)),
        }
    }
    for (tag, count) in &tally {
        if *count > 1 {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Error,
                    format!(
                        "Duplicate fvar axis tag '{tag}' \u{2014} axis tags must be unique within a font"
                    ),
                )
                .at(format!("fvar.axes.{tag}")),
            );
        }
    }
    for (idx, a) in axes.iter().enumerate() {
        let (min, default, max) = (
            fixed(a.min_value_raw),
            fixed(a.default_value_raw),
            fixed(a.max_value_raw),
        );
        if !(min <= default && default <= max) {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Error,
                    format!(
                        "fvar axis '{}' ({idx}) has invalid range: min={} default={} max={} (must be min \u{2264} default \u{2264} max)",
                        a.tag,
                        float_to_s(min),
                        float_to_s(default),
                        float_to_s(max)
                    ),
                )
                .at(format!("fvar.axes.{idx}")),
            );
        }
        if a.name_id == 0 {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Warning,
                    format!(
                        "fvar axis '{}' ({idx}) has no name ID \u{2014} users cannot identify the axis",
                        a.tag
                    ),
                )
                .at(format!("fvar.axes.{idx}.axis_name_id")),
            );
        }
    }

    // validate_instances
    for (idx, inst) in fvar.instances.iter().enumerate() {
        for (axis_idx, &raw) in inst.coordinates_raw.iter().enumerate() {
            let Some(axis) = axes.get(axis_idx) else {
                continue;
            };
            let (min, max) = (fixed(axis.min_value_raw), fixed(axis.max_value_raw));
            let coord = fixed(raw);
            if coord >= min && coord <= max {
                continue;
            }
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Warning,
                    format!(
                        "fvar instance {idx} has coordinate {} for axis '{}' outside its range [{}, {}]",
                        float_to_s(coord),
                        axis.tag,
                        float_to_s(min),
                        float_to_s(max)
                    ),
                )
                .at(format!("fvar.instances.{idx}.{}", axis.tag)),
            );
        }
    }

    // validate_companion_tables
    if font.has_table("glyf") && !font.has_table("gvar") {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                "Variable TrueType font has no 'gvar' table \u{2014} glyph outlines won't vary across the design space",
            )
            .at("tables.gvar"),
        );
    }
    if !font.has_table("HVAR") {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                "Variable font has no 'HVAR' table \u{2014} advance widths won't vary; renderers must fall back to per-glyph hmtx lookups",
            )
            .at("tables.HVAR"),
        );
    }
    if !font.has_table("STAT") {
        issues.push(
            Issue::new(
                CODE,
                Severity::Info,
                "Variable font has no 'STAT' table \u{2014} OS style-matching won't work; Windows and modern font pickers rely on STAT",
            )
            .at("tables.STAT"),
        );
    }
    Ok(issues)
}
