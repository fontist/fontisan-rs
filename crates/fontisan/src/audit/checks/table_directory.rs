//! `table_directory` (Ruby `TableDirectoryCheck`): searchRange/entrySelector/
//! rangeShift, 4-byte alignment, per-table checksums, offsets and lengths.

use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "table_directory";

fn largest_power_of_two_le(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    1 << (63 - n.leading_zeros())
}

/// Sum of big-endian u32 words over the zero-padded data, mod 2^32.
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    // Ruby reads search_range off the WOFF/WOFF2 header, which has none.
    let header = match font.format() {
        crate::sfnt::Format::Woff => Some("#<Fontisan::WoffHeader>"),
        crate::sfnt::Format::Woff2 => Some("#<Fontisan::Woff2::Woff2Header>"),
        _ => None,
    };
    if let Some(h) = header {
        return Err(CheckError::new(
            "NoMethodError",
            format!("undefined method 'search_range' for {h}"),
        ));
    }
    let (num_tables, search_range, entry_selector, range_shift) = font.directory_fields();
    let n = u64::from(num_tables);
    let mut issues = Vec::new();

    // validate_search_fields
    let expected_range = largest_power_of_two_le(n) * 16;
    if expected_range == 0 {
        // Math.log2(0).to_i
        return Err(CheckError::new("FloatDomainError", "-Infinity"));
    }
    let expected_selector = u64::from((expected_range / 16).trailing_zeros());
    let expected_shift = (n * 16) as i64 - expected_range as i64;
    if u64::from(search_range) != expected_range {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "searchRange is {search_range} but should be {expected_range} (numTables={n})"
                ),
            )
            .at("header.searchRange"),
        );
    }
    if u64::from(entry_selector) != expected_selector {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!("entrySelector is {entry_selector} but should be {expected_selector}"),
            )
            .at("header.entrySelector"),
        );
    }
    if i64::from(range_shift) != expected_shift {
        issues.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!("rangeShift is {range_shift} but should be {expected_shift}"),
            )
            .at("header.rangeShift"),
        );
    }

    // validate_alignment
    for e in font.entries() {
        if e.offset % 4 != 0 {
            let tag = e.tag_str();
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Warning,
                    format!("Table '{tag}' at offset {} is not 4-byte aligned", e.offset),
                )
                .at(format!("tables.{tag}.offset")),
            );
        }
    }

    // validate_checksums (`font.table_data[tag]`; head is skipped)
    for e in font.entries() {
        let tag = e.tag_str();
        let Some(raw) = font.table_data(&tag) else {
            continue;
        };
        if tag == "head" {
            continue;
        }
        let actual = checksum(raw);
        if actual != e.checksum {
            // Ruby builds the message with `entry.checksum.to_s(16)`; a BinData
            // Uint32be#to_s takes no argument, so the whole check dies here
            // (a known reference bug, docs/divergences.md) and the report
            // carries the fatal "failed to execute" issue instead.
            return Err(CheckError::new(
                "ArgumentError",
                "wrong number of arguments (given 1, expected 0)",
            ));
        }
    }

    // validate_offsets
    let dir_end = 12 + n * 16;
    for e in font.entries() {
        let tag = e.tag_str();
        if u64::from(e.offset) < dir_end {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Error,
                    format!(
                        "Table '{tag}' offset {} overlaps the table directory (directory ends at {dir_end})",
                        e.offset
                    ),
                )
                .at(format!("tables.{tag}.offset")),
            );
        }
        let mismatched = font
            .table_data(&tag)
            .filter(|raw| raw.len() != e.length as usize);
        if let Some(raw) = mismatched {
            issues.push(
                Issue::new(
                    CODE,
                    Severity::Error,
                    format!(
                        "Table '{tag}' length mismatch: directory says {} but actual data is {} bytes",
                        e.length,
                        raw.len()
                    ),
                )
                .at(format!("tables.{tag}.table_length")),
            );
        }
    }
    Ok(issues)
}
