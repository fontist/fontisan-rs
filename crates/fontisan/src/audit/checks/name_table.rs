//! `ot_name` (Ruby `NameTableCheck`).
use crate::audit::ruby::need;
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_name";

fn ps_name_ok(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_alphabetic())
        && c.all(|x| x.is_ascii_alphanumeric() || matches!(x, '.' | '_' | '-'))
}

/// `/\AVersion\s/i`
fn starts_with_version(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() > 7
        && b[..7].eq_ignore_ascii_case(b"version")
        && matches!(b[7], b' ' | b'\t' | b'\n' | b'\r' | 0x0c | 0x0b)
}

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("name") {
        return Ok(vec![]);
    }
    let name = need(font.name(), "english_name")?;
    let mut v = Vec::new();
    for (id, label) in [
        (1u16, "Family"),
        (2, "Subfamily"),
        (4, "Full"),
        (6, "PostScript"),
    ] {
        if name.english_name(id).unwrap_or_default().is_empty() {
            v.push(
                Issue::new(
                    CODE,
                    Severity::Error,
                    format!("Required name ID {id} ({label}) is missing from the name table"),
                )
                .at(format!("name.nameID.{id}")),
            );
        }
    }
    let ps = name.english_name(6).unwrap_or_default();
    if !ps.is_empty() && !ps_name_ok(&ps) {
        v.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!("PostScript name '{ps}' contains characters outside the OT spec grammar (must start with a letter, then A\u{2013}Z a\u{2013}z 0\u{2013}9 . - _)"),
            )
            .at("name.nameID.6"),
        );
    }
    let ver = name.english_name(5).unwrap_or_default();
    if !ver.is_empty() && !starts_with_version(&ver) {
        v.push(
            Issue::new(
                CODE,
                Severity::Info,
                format!("name ID 5 (version) is '{ver}' but should start with 'Version ' per OT spec convention"),
            )
            .at("name.nameID.5"),
        );
    }
    Ok(v)
}
