//! `format_round_trip` (Ruby `FormatRoundTripCheck`): convert glyf fonts to OTF and
//! CFF/CFF2 fonts to TTF, reload the result and compare glyph count and codepoints.
//!
//! Ruby's `convert_to_temp` rescues every `StandardError` and returns nil, so a
//! conversion that raises (or yields no tables) reports nothing. Only a failure
//! to reload the converted file reaches the outer rescue and becomes the
//! `Round-trip conversion to X failed: <Class>: <message>` warning.
use std::collections::BTreeSet;

use crate::audit::types::{CheckError, Issue, Severity};
use crate::convert::{self, Target};
use crate::sfnt::Font;

pub const CODE: &str = "format_round_trip";

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    let target = if font.has_table("glyf") {
        Target::Otf
    } else if font.has_table("CFF ") || font.has_table("CFF2") {
        Target::Ttf
    } else {
        return Ok(vec![]);
    };
    let Ok(converted) = convert::convert(&sfnt_bytes(font), target, 0) else {
        return Ok(vec![]);
    };
    Ok(compare(font, &converted, target))
}

/// The face's tables as a plain sfnt (what `convert` consumes), keeping the
/// directory order and a repeated tag's last data like a Ruby hash.
fn sfnt_bytes(font: &Font<'_>) -> Vec<u8> {
    let mut tables: Vec<(String, &[u8])> = Vec::new();
    for e in font.entries() {
        let tag = e.tag_str();
        let data = font.table_data(&tag).unwrap_or(&[]);
        match tables.iter_mut().find(|(t, _)| *t == tag) {
            Some(slot) => slot.1 = data,
            None => tables.push((tag, data)),
        }
    }
    let mut out = Vec::new();
    out.extend(font.sfnt_version.to_be_bytes());
    out.extend((tables.len() as u16).to_be_bytes());
    out.extend([0u8; 6]);
    let mut off = 12 + 16 * tables.len();
    for (tag, d) in &tables {
        let mut t = [b' '; 4];
        for (i, b) in tag.bytes().take(4).enumerate() {
            t[i] = b;
        }
        out.extend(t);
        out.extend(0u32.to_be_bytes());
        out.extend((off as u32).to_be_bytes());
        out.extend((d.len() as u32).to_be_bytes());
        off += d.len();
    }
    for (_, d) in &tables {
        out.extend(*d);
    }
    out
}

fn label(target: Target) -> &'static str {
    if target == Target::Otf { "OTF" } else { "TTF" }
}

fn glyph_count(font: &Font<'_>) -> u64 {
    if !font.has_table("maxp") {
        return 0;
    }
    font.maxp()
        .ok()
        .flatten()
        .map_or(0, |m| u64::from(m.num_glyphs))
}

fn codepoints(font: &Font<'_>) -> BTreeSet<u32> {
    if !font.has_table("cmap") {
        return BTreeSet::new();
    }
    font.cmap()
        .ok()
        .flatten()
        .map(|c| c.codepoints().into_iter().collect())
        .unwrap_or_default()
}

fn first_five(set: impl Iterator<Item = u32>) -> String {
    set.take(5)
        .map(|c| format!("U+{c:04X}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn compare(source: &Font<'_>, converted: &[u8], target: Target) -> Vec<Issue> {
    let t = label(target);
    let loc = format!("round_trip.{}", t.to_lowercase());
    let reloaded = match Font::from_bytes(converted) {
        Ok(f) => f,
        Err(e) => return vec![reload_failure(t, e.class.ruby_name(), &e.message)],
    };
    let mut issues = Vec::new();
    let (src, dst) = (glyph_count(source), glyph_count(&reloaded));
    if src != dst {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!("Round-trip to {t}: glyph count changed from {src} to {dst}"),
            )
            .at(format!("{loc}.glyph_count")),
        );
    }
    let (src_cps, dst_cps) = (codepoints(source), codepoints(&reloaded));
    let missing: Vec<u32> = src_cps.difference(&dst_cps).copied().collect();
    let extra: Vec<u32> = dst_cps.difference(&src_cps).copied().collect();
    if !missing.is_empty() {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "Round-trip to {t}: {} codepoints lost (first: {})",
                    missing.len(),
                    first_five(missing.iter().copied())
                ),
            )
            .at(format!("{loc}.codepoints.missing")),
        );
    }
    if !extra.is_empty() {
        issues.push(
            Issue::new(
                CODE,
                Severity::Info,
                format!(
                    "Round-trip to {t}: {} codepoints gained (first: {})",
                    extra.len(),
                    first_five(extra.iter().copied())
                ),
            )
            .at(format!("{loc}.codepoints.extra")),
        );
    }
    issues
}

fn reload_failure(t: &str, class: &str, message: &str) -> Issue {
    Issue::new(
        CODE,
        Severity::Warning,
        format!("Round-trip conversion to {t} failed: {class}: {message}"),
    )
    .at(format!("round_trip.{}", t.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_failure_has_ruby_message_shape() {
        let i = reload_failure("OTF", "Fontisan::InvalidFontError", "boom");
        assert_eq!(
            i.message,
            "Round-trip conversion to OTF failed: Fontisan::InvalidFontError: boom"
        );
        assert_eq!(i.location.as_deref(), Some("round_trip.otf"));
        assert_eq!(i.severity, Severity::Warning);
        assert_eq!(i.category, "format_round_trip");
    }

    #[test]
    fn unreadable_converted_font_is_reported_and_conversion_errors_are_silent() {
        // glyf font that cannot be converted: Ruby's convert_to_temp rescues -> no issue.
        let mut b = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        b.extend(b"glyf");
        b.extend(0u32.to_be_bytes());
        b.extend(28u32.to_be_bytes());
        b.extend(0u32.to_be_bytes());
        let f = Font::from_bytes(&b).unwrap();
        assert!(run(&f).unwrap().is_empty());
        // reload of garbage bytes surfaces the loader's class and message.
        let issues = compare(&f, b"garbage!", Target::Otf);
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0]
                .message
                .starts_with("Round-trip conversion to OTF failed: Fontisan::InvalidFontError: "),
            "{}",
            issues[0].message
        );
    }
}
