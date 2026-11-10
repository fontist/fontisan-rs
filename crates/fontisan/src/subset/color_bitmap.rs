//! CBDT / CBLC subsetting, a port of Ruby's `ColorBitmapSubsetter` and its
//! plan classes (`ColorBitmapSubsetPlan`, `ColorBitmapStrikePlan`,
//! `ColorBitmapSubtablePlan`).
//!
//! Both tables come out of one pass: every strike keeps its BitmapSize record
//! (only the array offset and subtable count are patched), every surviving
//! glyph's bitmap block is copied into a fresh CBDT, and each strike's
//! surviving glyphs are re-indexed as format 1 IndexSubTables, one per
//! contiguous run of new glyph ids. A strike without a survivor is dropped.
//! Quirks of the Ruby code are kept on purpose: a block that does not fit in
//! the source CBDT is skipped but still counted in the offset array, the
//! image format of a run is its first glyph's, and `start/endGlyphIndex` stay
//! the source's.

use super::{Mapping, SubsetError};

const BITMAP_SIZE_LEN: usize = 48;

fn be16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Ruby's `bytes[offset, count * width].unpack("N*")` (or `"n*"`): nil past the
/// end raises, a short slice drops the trailing partial value.
fn unpack(
    bytes: &[u8],
    offset: usize,
    count: usize,
    width: usize,
) -> Result<Vec<i64>, SubsetError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if offset > bytes.len() {
        return Err(SubsetError::new(
            "NoMethodError",
            "undefined method 'unpack' for nil",
        ));
    }
    let end = bytes.len().min(offset.saturating_add(count * width));
    Ok(bytes[offset..end]
        .chunks_exact(width)
        .map(|c| c.iter().fold(0i64, |a, &b| (a << 8) | i64::from(b)))
        .collect())
}

struct Location {
    glyph_id: u32,
    image_format: u16,
    cbdt_offset: i64,
    byte_length: i64,
}

/// `CblcIndexSubTableFormatParser.locations` over one IndexSubTable.
fn locations(
    cblc: &[u8],
    base: usize,
    first: u32,
    last: u32,
) -> Result<Vec<Location>, SubsetError> {
    let corrupt = || SubsetError::corrupted("CBLC IndexSubTable is truncated");
    let index_format = be16(cblc, base).ok_or_else(corrupt)?;
    let image_format = be16(cblc, base + 2).ok_or_else(corrupt)?;
    let image_data_offset = i64::from(be32(cblc, base + 4).ok_or_else(corrupt)?);
    let count = (i64::from(last) - i64::from(first) + 1).max(0) as usize;
    let built = |offsets: &[i64]| -> Vec<Location> {
        (0..count)
            .map(|i| {
                let start = offsets.get(i).copied().unwrap_or(0);
                let end = offsets.get(i + 1).copied().unwrap_or(start);
                Location {
                    glyph_id: first + i as u32,
                    image_format,
                    cbdt_offset: image_data_offset + start,
                    byte_length: end - start,
                }
            })
            .collect()
    };
    match index_format {
        1 => Ok(built(&unpack(cblc, base + 8, count + 1, 4)?)),
        3 => {
            let raw = unpack(cblc, base + 8, count + 1, 2)?;
            Ok(built(&raw.iter().map(|v| v * 4).collect::<Vec<_>>()))
        }
        2 => {
            let size = unpack(cblc, base + 8, 1, 4)?
                .first()
                .copied()
                .ok_or_else(|| {
                    SubsetError::new("NoMethodError", "undefined method '[]' for nil")
                })?;
            Ok((0..count)
                .map(|i| Location {
                    glyph_id: first + i as u32,
                    image_format,
                    cbdt_offset: image_data_offset + i as i64 * size,
                    byte_length: size,
                })
                .collect())
        }
        5 => {
            let size = unpack(cblc, base + 8, 1, 4)?
                .first()
                .copied()
                .ok_or_else(|| {
                    SubsetError::new("NoMethodError", "undefined method '[]' for nil")
                })?;
            let offsets = unpack(cblc, base + 12 + 8 + 4, count + 1, 4)?;
            Ok((0..count)
                .map(|i| Location {
                    glyph_id: first + i as u32,
                    image_format,
                    cbdt_offset: image_data_offset + offsets.get(i).copied().unwrap_or(0),
                    byte_length: size,
                })
                .collect())
        }
        4 => Err(SubsetError::new(
            "Fontisan::Tables::CblcIndexSubTableFormatParser::UnsupportedFormat",
            "CBLC IndexSubTable format 4 (bit-packed) is unsupported",
        )),
        f => Err(SubsetError::new(
            "Fontisan::Tables::CblcIndexSubTableFormatParser::UnsupportedFormat",
            format!("Unknown CBLC IndexSubTable format: {f}"),
        )),
    }
}

struct Placement {
    new_gid: u32,
    source_offset: i64,
    byte_length: i64,
    new_offset: u32,
}

/// One format 1 IndexSubTable: a run of consecutive new glyph ids.
struct Subtable {
    image_format: u16,
    placements: Vec<usize>,
}

struct StrikePlan<'a> {
    source: &'a [u8],
    subtables: Vec<Subtable>,
}

impl Subtable {
    fn byte_length(&self) -> usize {
        8 + (self.placements.len() + 1) * 4
    }
}

/// Subset CBDT and CBLC bytes `(cbdt, cblc)`. Both are empty when the font has
/// only one of the two source tables (as in Ruby).
pub(super) fn subset(
    cbdt: Option<&[u8]>,
    cblc: Option<&[u8]>,
    mapping: &Mapping,
) -> Result<(Vec<u8>, Vec<u8>), SubsetError> {
    let (Some(cbdt), Some(cblc)) = (cbdt, cblc) else {
        return Ok((Vec::new(), Vec::new()));
    };
    let corrupt = || SubsetError::corrupted("CBLC table is truncated");
    let version = be32(cblc, 0).ok_or_else(corrupt)?;
    let num_sizes = be32(cblc, 4).ok_or_else(corrupt)? as usize;

    // collect!
    let mut placements: Vec<Placement> = Vec::new();
    let mut strikes: Vec<StrikePlan> = Vec::new();
    for i in 0..num_sizes {
        let at = 8 + i * BITMAP_SIZE_LEN;
        let source = cblc.get(at..at + BITMAP_SIZE_LEN).ok_or_else(corrupt)?;
        let array_offset = be32(source, 0).unwrap_or(0) as usize;
        let n_subs = be32(source, 8).unwrap_or(0) as usize;
        let mut plan = StrikePlan {
            source,
            subtables: Vec::new(),
        };
        // sub_tables_for: no subtables, or an array that is out of range, is empty
        let entries = if n_subs == 0 {
            None
        } else {
            cblc.get(array_offset..)
                .map(|rest| &rest[..rest.len().min(n_subs * 8)])
        };
        for e in 0..n_subs {
            let Some(entry) = entries.and_then(|b| b.get(e * 8..e * 8 + 8)) else {
                return Err(corrupt());
            };
            let first = u32::from(be16(entry, 0).unwrap_or(0));
            let last = u32::from(be16(entry, 2).unwrap_or(0));
            let base = array_offset + be32(entry, 4).unwrap_or(0) as usize;
            for loc in locations(cblc, base, first, last)? {
                let Some(new_gid) = mapping.new_id(loc.glyph_id) else {
                    continue;
                };
                placements.push(Placement {
                    new_gid,
                    source_offset: loc.cbdt_offset,
                    byte_length: loc.byte_length,
                    new_offset: 0,
                });
                let idx = placements.len() - 1;
                let joins = plan
                    .subtables
                    .last()
                    .and_then(|s| s.placements.last())
                    .is_some_and(|&p| new_gid == placements[p].new_gid.wrapping_add(1));
                if joins {
                    plan.subtables.last_mut().unwrap().placements.push(idx);
                } else {
                    plan.subtables.push(Subtable {
                        image_format: loc.image_format,
                        placements: vec![idx],
                    });
                }
            }
        }
        if !plan.subtables.is_empty() {
            strikes.push(plan);
        }
    }

    // emit_cbdt
    let mut out_cbdt: Vec<u8> = cbdt[..cbdt.len().min(4)].to_vec();
    for p in &mut placements {
        let (off, len) = (p.source_offset, p.byte_length);
        if off < 0 || len < 0 || off + len > cbdt.len() as i64 {
            continue;
        }
        p.new_offset = out_cbdt.len() as u32;
        out_cbdt.extend_from_slice(&cbdt[off as usize..(off + len) as usize]);
    }

    // emit_cblc
    let mut out = Vec::new();
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&(strikes.len() as u32).to_be_bytes());
    let mut ista_offsets = Vec::new();
    let mut cursor = out.len() + strikes.len() * BITMAP_SIZE_LEN;
    for s in &strikes {
        ista_offsets.push(cursor);
        cursor += s.subtables.len() * 8;
    }
    let ista_section_end = ista_offsets.last().copied().unwrap_or(0)
        + strikes.iter().map(|s| s.subtables.len()).sum::<usize>() * 8;
    for (s, &ista) in strikes.iter().zip(&ista_offsets) {
        out.extend_from_slice(&(ista as u32).to_be_bytes());
        out.extend_from_slice(&s.source[4..8]);
        out.extend_from_slice(&(s.subtables.len() as u32).to_be_bytes());
        out.extend_from_slice(&s.source[12..]);
    }
    let mut ist_cursor = ista_section_end;
    for (s, &ista) in strikes.iter().zip(&ista_offsets) {
        for sub in &s.subtables {
            let first = placements[sub.placements[0]].new_gid;
            let last = placements[*sub.placements.last().unwrap()].new_gid;
            out.extend_from_slice(&(first as u16).to_be_bytes());
            out.extend_from_slice(&(last as u16).to_be_bytes());
            out.extend_from_slice(&((ist_cursor - ista) as u32).to_be_bytes());
            ist_cursor += sub.byte_length();
        }
    }
    for s in &strikes {
        for sub in &s.subtables {
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&sub.image_format.to_be_bytes());
            out.extend_from_slice(&placements[sub.placements[0]].new_offset.to_be_bytes());
            let mut acc: i64 = 0;
            out.extend_from_slice(&(acc as u32).to_be_bytes());
            for &p in &sub.placements {
                acc += placements[p].byte_length;
                out.extend_from_slice(&(acc as u32).to_be_bytes());
            }
        }
    }
    Ok((out_cbdt, out))
}
