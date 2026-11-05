//! `glyph_names` (Ruby `GlyphNameCheck`, file `glyph_name_check.rb`): length,
//! character grammar, leading digit and duplicate glyph names from `post`.

use super::rubyfmt::sfnt_err;
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "glyph_names";
const MAX_NAME_LENGTH: usize = 63;

fn allowed(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("post") {
        return Ok(Vec::new());
    }
    // `post = font.table("post"); return [] unless post`
    let names = match font.post() {
        Ok(Some(post)) => post.glyph_names(),
        Ok(None) => return Ok(Vec::new()),
        Err(e) => return Err(sfnt_err(e)),
    };
    let mut issues = Vec::new();
    for (gid, name) in names.iter().enumerate() {
        issues.extend(check_name(name, gid));
    }
    issues.extend(check_duplicates(&names));
    Ok(issues)
}

fn check_name(name: &str, gid: usize) -> Vec<Issue> {
    let mut issues = Vec::new();
    if name.is_empty() {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!("Glyph at GID {gid} has an empty name"),
            )
            .at(format!("glyph.{gid}.name")),
        );
        return issues;
    }
    let len = name.chars().count();
    if len > MAX_NAME_LENGTH {
        let head: String = name.chars().take(20).collect();
        issues.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!(
                    "Glyph name '{head}\u{2026}' (GID {gid}) exceeds {MAX_NAME_LENGTH} characters ({len})"
                ),
            )
            .at(format!("glyph.{gid}.name")),
        );
    }
    if !allowed(name) {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "Glyph name '{name}' (GID {gid}) contains characters outside the OT spec grammar (A\u{2013}Z a\u{2013}z 0\u{2013}9 . - _)"
                ),
            )
            .at(format!("glyph.{gid}.name")),
        );
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "Glyph name '{name}' (GID {gid}) starts with a digit \u{2014} not portable"
                ),
            )
            .at(format!("glyph.{gid}.name")),
        );
    }
    issues
}

/// Duplicates in first-occurrence order (Ruby hash insertion order).
fn check_duplicates(names: &[String]) -> Vec<Issue> {
    let mut order: Vec<&str> = Vec::new();
    let mut gids: std::collections::HashMap<&str, Vec<usize>> = Default::default();
    for (gid, name) in names.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        gids.entry(name)
            .or_insert_with(|| {
                order.push(name);
                Vec::new()
            })
            .push(gid);
    }
    order
        .into_iter()
        .filter_map(|name| {
            let g = &gids[name];
            (g.len() > 1).then(|| {
                let first: Vec<String> = g.iter().take(5).map(usize::to_string).collect();
                let more = if g.len() > 5 { "\u{2026}" } else { "" };
                Issue::new(
                    CODE,
                    Severity::Warning,
                    format!(
                        "Glyph name '{name}' is shared by {} glyphs (GIDs: {}{more})",
                        g.len(),
                        first.join(", ")
                    ),
                )
                .at(format!("glyph_names.{name}"))
            })
        })
        .collect()
}
