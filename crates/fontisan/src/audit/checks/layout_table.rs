//! `ot_layout` (Ruby `LayoutTableCheck`).
use crate::audit::ruby::check_error;
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_layout";

fn one(font: &Font<'_>, tag: &str, out: &mut Vec<Issue>) -> Result<(), CheckError> {
    if !font.has_table(tag) {
        return Ok(());
    }
    let table = if tag == "GSUB" {
        font.gsub()
    } else {
        font.gpos()
    };
    let Some(table) = table.map_err(check_error)? else {
        return Ok(());
    };
    let scripts = table.scripts();
    let lower = tag.to_lowercase();
    if scripts.is_empty() {
        out.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!("{tag} table has no script records \u{2014} layout rules won't apply without at least one script (DFLT recommended)"),
            )
            .at(format!("{lower}.script_list")),
        );
    }
    if !scripts.contains(b"DFLT") {
        out.push(
            Issue::new(
                CODE,
                Severity::Info,
                format!("{tag} table has no 'DFLT' script \u{2014} fonts without DFLT may not apply features to scripts not explicitly listed"),
            )
            .at(format!("{lower}.script_list.DFLT")),
        );
    }
    Ok(())
}

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    let mut v = Vec::new();
    one(font, "GSUB", &mut v)?;
    one(font, "GPOS", &mut v)?;
    Ok(v)
}
