//! `naming_consistency` (Ruby `NamingConsistencyCheck`): PostScript-name prefix,
//! full-name composition, and standard subfamily names.

use super::rubyfmt::required;
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::{Font, Name};

pub const CODE: &str = "naming_consistency";

const FAMILY: u16 = 1;
const SUBFAMILY: u16 = 2;
const FULL_NAME: u16 = 4;
const POSTSCRIPT_NAME: u16 = 6;
const RECOGNIZED_SUBFAMILIES: [&str; 14] = [
    "Regular",
    "Italic",
    "Bold",
    "BoldItalic",
    "Thin",
    "Light",
    "Medium",
    "SemiBold",
    "Black",
    "Condensed",
    "Extended",
    "Oblique",
    "Book",
    "Normal",
];

/// `name.english_name(id).to_s`.
fn english(name: &Name, id: u16) -> String {
    name.english_name(id).unwrap_or_default()
}

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("name") {
        return Ok(Vec::new());
    }
    let name = required(font.name(), "english_name")?;
    let mut issues = Vec::new();

    let family = english(&name, FAMILY);
    let ps = english(&name, POSTSCRIPT_NAME);
    if !family.is_empty() && !ps.is_empty() {
        let compact = family.replace(' ', "");
        if !ps.starts_with(&compact) {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Warning,
                    format!(
                        "PostScript name '{ps}' does not start with the family name '{family}' (expected prefix: '{compact}')"
                    ),
                )
                .at("name.consistency.ps_prefix"),
            );
        }
    }

    let subfamily = english(&name, SUBFAMILY);
    let full = english(&name, FULL_NAME);
    if !family.is_empty() && !subfamily.is_empty() && !full.is_empty() {
        let expected = format!("{family} {subfamily}");
        if full != expected {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Info,
                    format!("Full name '{full}' doesn't match Family + Subfamily '{expected}'"),
                )
                .at("name.consistency.full_name"),
            );
        }
    }

    if !font.has_table("fvar") && !subfamily.is_empty() && !recognized(&subfamily) {
        issues.push(
            Issue::new(
                CODE,
                Severity::Info,
                format!(
                    "Subfamily '{subfamily}' is not a standard style name \u{2014} font pickers may not categorize it correctly"
                ),
            )
            .at("name.consistency.subfamily"),
        );
    }
    Ok(issues)
}

/// `casecmp?` against the recognized list (Ruby's `delete('"')` is a no-op on it).
fn recognized(subfamily: &str) -> bool {
    let s = subfamily.to_lowercase();
    RECOGNIZED_SUBFAMILIES.iter().any(|r| s == r.to_lowercase())
}
