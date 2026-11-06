//! `cmap` (Ruby `CmapCheck`): header, Unicode presence and per-subtable
//! invariants over the raw table bytes, including the reference's own quirks
//! (the format 12 header is unpacked with `"nnNnN"`, so `numGroups` is read from
//! bytes 10..14; an encoding-record array that does not fit makes
//! `Array.new { break }` return nil and the check raise `NoMethodError`).

use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "cmap";

fn u16_at(raw: &[u8], at: usize) -> u64 {
    u64::from(u16::from_be_bytes([raw[at], raw[at + 1]]))
}

fn u32_at(raw: &[u8], at: usize) -> u64 {
    u64::from(u32::from_be_bytes([
        raw[at],
        raw[at + 1],
        raw[at + 2],
        raw[at + 3],
    ]))
}

fn issue(sev: Severity, msg: String, loc: String) -> Issue {
    Issue::new(CODE, sev, msg).at(loc)
}

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("cmap") {
        return Ok(Vec::new());
    }
    let Some(raw) = font.table_data("cmap") else {
        return Ok(Vec::new());
    };
    let header = validate_header(raw);
    if !header.is_empty() {
        return Ok(header);
    }
    let num_tables = u16_at(raw, 2) as usize;
    // read_encoding_records: `break` inside Array.new makes the whole array nil.
    if 4 + 8 * num_tables > raw.len() {
        return Err(CheckError::new(
            "NoMethodError",
            "undefined method 'any?' for nil",
        ));
    }
    let records: Vec<(usize, u64, u64, u64)> = (0..num_tables)
        .map(|i| {
            let o = 4 + 8 * i;
            (i, u16_at(raw, o), u16_at(raw, o + 2), u32_at(raw, o + 4))
        })
        .collect();
    let mut issues = Vec::new();
    let has_unicode = records
        .iter()
        .any(|&(_, p, e, _)| p == 0 || (p == 3 && (e == 1 || e == 10)));
    if !has_unicode {
        issues.push(issue(
            Severity::Warning,
            "cmap has no Unicode-encoded subtable (expected platform 0 or 3 with encoding 1 or 10)"
                .into(),
            "cmap.encoding_records".into(),
        ));
    }
    for &(index, _, _, off) in &records {
        issues.extend(validate_subtable(raw, index, off as usize));
    }
    Ok(issues)
}

fn validate_header(raw: &[u8]) -> Vec<Issue> {
    if raw.len() < 4 {
        return vec![issue(
            Severity::Error,
            format!(
                "cmap table is {} bytes, shorter than its 4-byte header",
                raw.len()
            ),
            "cmap.header".into(),
        )];
    }
    let mut issues = Vec::new();
    let (version, num_tables) = (u16_at(raw, 0), u16_at(raw, 2));
    if version != 0 {
        issues.push(issue(
            Severity::Error,
            format!("cmap version is {version} but must be 0"),
            "cmap.version".into(),
        ));
    }
    if num_tables == 0 {
        issues.push(issue(
            Severity::Error,
            "cmap has no encoding records".into(),
            "cmap.numTables".into(),
        ));
    }
    issues
}

fn validate_subtable(raw: &[u8], index: usize, off: usize) -> Vec<Issue> {
    let loc = format!("cmap.encoding_records.{index}");
    if off + 2 > raw.len() {
        return vec![issue(
            Severity::Error,
            format!("cmap subtable at record {index} has offset {off} beyond the table end"),
            loc,
        )];
    }
    match u16_at(raw, off) {
        0 => {
            if off + 262 > raw.len() {
                vec![issue(
                    Severity::Error,
                    format!(
                        "cmap format 0 (record {index}): subtable is truncated (need 262 bytes)"
                    ),
                    loc,
                )]
            } else {
                Vec::new()
            }
        }
        4 => format4(raw, off, index, &loc),
        6 => {
            if off + 10 > raw.len() {
                return Vec::new();
            }
            let (first, count) = (u16_at(raw, off + 6), u16_at(raw, off + 8));
            if off + 10 + (count as usize) * 2 > raw.len() {
                vec![issue(
                    Severity::Error,
                    format!(
                        "cmap format 6 (record {index}): subtable truncated (first={first}, count={count})"
                    ),
                    loc,
                )]
            } else {
                Vec::new()
            }
        }
        12 => format12(raw, off, index, &loc),
        14 => Vec::new(),
        other => vec![issue(
            Severity::Warning,
            format!("cmap subtable at record {index} uses unknown format {other}"),
            loc,
        )],
    }
}

fn format4(raw: &[u8], off: usize, index: usize, loc: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    if off + 14 > raw.len() {
        return issues;
    }
    let seg_count_x2 = u16_at(raw, off + 6) as usize;
    let seg_count = seg_count_x2 / 2;
    if seg_count_x2 % 2 == 1 {
        issues.push(issue(
            Severity::Error,
            format!(
                "cmap format 4 (record {index}): segCountX2={seg_count_x2} is inconsistent with segCount={seg_count}"
            ),
            loc.into(),
        ));
    }
    let end_codes_start = off + 14;
    if seg_count > 0 && end_codes_start + seg_count * 2 <= raw.len() {
        let last_end = u16_at(raw, end_codes_start + (seg_count - 1) * 2);
        if last_end != 0xFFFF {
            issues.push(issue(
                Severity::Warning,
                format!(
                    "cmap format 4 (record {index}): last segment endCode is 0x{last_end:x} but should be 0xFFFF (sentinel)"
                ),
                loc.into(),
            ));
        }
    }
    let pad_off = end_codes_start + seg_count * 2;
    if pad_off + 2 <= raw.len() {
        let pad = u16_at(raw, pad_off);
        if pad != 0 {
            issues.push(issue(
                Severity::Error,
                format!("cmap format 4 (record {index}): reservedPad is {pad} but must be 0"),
                loc.into(),
            ));
        }
    }
    issues
}

fn format12(raw: &[u8], off: usize, index: usize, loc: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    if off + 16 > raw.len() {
        return issues;
    }
    // unpack("nnNnN"): numGroups is the N at bytes 10..14 (a reference bug).
    let num_groups = u32_at(raw, off + 10) as usize;
    let groups_start = off + 16;
    if num_groups == 0 || groups_start + num_groups * 12 > raw.len() {
        return issues;
    }
    let mut prev_end: Option<u64> = None;
    for g in 0..num_groups {
        let at = groups_start + g * 12;
        let (start, end) = (u32_at(raw, at), u32_at(raw, at + 4));
        if start > end {
            issues.push(issue(
                Severity::Error,
                format!(
                    "cmap format 12 (record {index}): group {g} has startCharCode 0x{start:x} > endCharCode 0x{end:x}"
                ),
                loc.into(),
            ));
        }
        if let Some(prev) = prev_end.filter(|&p| start <= p) {
            issues.push(issue(
                Severity::Error,
                format!(
                    "cmap format 12 (record {index}): group {g} overlaps or is out of order (startCharCode 0x{start:x} \u{2264} previous endCharCode 0x{prev:x})"
                ),
                loc.into(),
            ));
        }
        prev_end = Some(end);
    }
    issues
}
