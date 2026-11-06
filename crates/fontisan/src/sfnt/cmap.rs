//! `cmap` as Ruby reads it (`Fontisan::Tables::Cmap`): ONE subtable chosen by
//! (3,10), then (3,1), then the first platform-0 record, only formats 4 and 12
//! are decoded, and gid 0 is dropped. read-fonts decodes the subtables; the
//! selection is Ruby's own, not skrifa's `Charmap` (docs/substrate.md, 1b).
use super::cursor::Cursor;
use super::error::Error;
use std::collections::BTreeMap;

const MAX_CODEPOINT: u32 = 0x10_FFFF;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cmap {
    pub version: u16,
    pub num_tables: u16,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Record {
    platform_id: u16,
    encoding_id: u16,
    offset: u32,
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Ruby's `parse_format_4`: the declared `length` (not the table's end) bounds the
/// four segment arrays, a subtable declaring more than the cmap holds is dropped, and
/// a segment whose arrays were cut short by that bound is skipped (Ruby raises
/// NoMethodError there; the fix makes it a skip on both tiers).
fn format4(d: &[u8], m: &mut BTreeMap<u32, u32>) {
    if d.len() < 14 {
        return;
    }
    let (Some(length), Some(x2)) = (be16(d, 2), be16(d, 6)) else {
        return;
    };
    let length = length as usize;
    if length > d.len() {
        return;
    }
    let seg_count = x2 as usize / 2;
    let read_array = |start: usize| -> Vec<u16> {
        (0..seg_count)
            .map(|i| start + i * 2)
            .take_while(|o| o + 2 <= length)
            .filter_map(|o| be16(d, o))
            .collect()
    };
    let ends = read_array(14);
    // reservedPad sits after the end codes, whether or not they were all read.
    let starts = read_array(14 + seg_count * 2 + 2);
    let deltas = read_array(14 + seg_count * 4 + 2);
    let range_pos = 14 + seg_count * 6 + 2;
    let ranges = read_array(range_pos);
    for i in 0..seg_count {
        let (Some(&start), Some(&end), Some(&delta), Some(&range)) =
            (starts.get(i), ends.get(i), deltas.get(i), ranges.get(i))
        else {
            continue;
        };
        if start == 0xFFFF {
            continue;
        }
        for code in start..=end {
            let glyph = if range == 0 {
                code.wrapping_add(delta)
            } else {
                let at = range_pos + i * 2 + range as usize + (code - start) as usize * 2;
                if at + 2 > length {
                    continue;
                }
                match be16(d, at) {
                    Some(0) | None => continue,
                    Some(g) => g.wrapping_add(delta),
                }
            };
            if glyph != 0 {
                m.insert(code as u32, glyph as u32);
            }
        }
    }
}

/// Ruby's `parse_format_12`: a declared `length` past the cmap drops the subtable,
/// groups are read while they fit in `length`, and a group's end is clamped to
/// U+10FFFF.
fn format12(d: &[u8], m: &mut BTreeMap<u32, u32>) {
    if d.len() < 16 {
        return;
    }
    let (Some(length), Some(groups)) = (be32(d, 4), be32(d, 12)) else {
        return;
    };
    let length = length as usize;
    if length > d.len() {
        return;
    }
    let mut off = 16usize;
    for _ in 0..groups {
        if off + 12 > length {
            break;
        }
        let (Some(start), Some(end), Some(gid)) =
            (be32(d, off), be32(d, off + 4), be32(d, off + 8))
        else {
            break;
        };
        for code in start..=end.min(MAX_CODEPOINT) {
            let glyph = gid as u64 + (code - start) as u64;
            if glyph != 0 {
                m.insert(code, glyph.min(u32::MAX as u64) as u32);
            }
        }
        off += 12;
    }
}

impl Cmap {
    pub(crate) fn parse(raw: &[u8]) -> Result<Self, Error> {
        if raw.is_empty() {
            return Ok(Self::default());
        }
        let mut c = Cursor::new(raw, 0);
        Ok(Self {
            version: c.u16()?,
            num_tables: c.u16()?,
            data: raw.to_vec(),
        })
    }

    /// Encoding records, stopping at the first one that does not fit.
    fn records(&self) -> Vec<Record> {
        let mut out = Vec::new();
        let mut off = 4;
        for _ in 0..self.num_tables {
            if off + 8 > self.data.len() {
                break;
            }
            let d = &self.data[off..off + 8];
            out.push(Record {
                platform_id: u16::from_be_bytes([d[0], d[1]]),
                encoding_id: u16::from_be_bytes([d[2], d[3]]),
                offset: u32::from_be_bytes([d[4], d[5], d[6], d[7]]),
            });
            off += 8;
        }
        out
    }

    /// The record's subtable bytes, `None` when the offset is past the table.
    fn subtable(&self, r: &Record) -> Option<&[u8]> {
        let off = r.offset as usize;
        (off < self.data.len()).then(|| &self.data[off..])
    }

    /// Distinct subtable formats over all records, ascending.
    pub fn subtable_formats(&self) -> Vec<u16> {
        let mut f: Vec<u16> = self
            .records()
            .iter()
            .filter_map(|r| self.subtable(r))
            .filter(|s| s.len() >= 2)
            .map(|s| u16::from_be_bytes([s[0], s[1]]))
            .collect();
        f.sort_unstable();
        f.dedup();
        f
    }

    /// Sorted unique codepoints mapped to a nonzero glyph by the chosen subtable.
    pub fn codepoints(&self) -> Vec<u32> {
        self.mappings().into_iter().map(|(c, _)| c).collect()
    }

    /// `Cmap#unicode_mappings`: (codepoint, glyph) pairs of the chosen subtable
    /// with a nonzero glyph, sorted by codepoint, one pair per codepoint.
    pub fn mappings(&self) -> Vec<(u32, u32)> {
        let recs = self.records();
        let pick = |p: u16, e: Option<u16>| {
            recs.iter()
                .find(|r| r.platform_id == p && e.is_none_or(|e| r.encoding_id == e))
                .and_then(|r| self.subtable(r))
        };
        let Some(sub) = pick(3, Some(10))
            .or_else(|| pick(3, Some(1)))
            .or_else(|| pick(0, None))
        else {
            return Vec::new();
        };
        let mut m: BTreeMap<u32, u32> = BTreeMap::new();
        match sub.get(0..2).map(|b| u16::from_be_bytes([b[0], b[1]])) {
            Some(4) => format4(sub, &mut m),
            Some(12) => format12(sub, &mut m),
            _ => {}
        }
        m.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fuzz finding (1j-parsers): one format 12 group spanning the whole u32 range
    /// made `codepoints()` run for minutes and allocate without bound.
    #[test]
    fn format_12_group_spanning_u32_is_bounded() {
        let mut raw = vec![0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12];
        raw.extend_from_slice(&[0, 12, 0, 0]); // format 12, reserved
        raw.extend_from_slice(&28u32.to_be_bytes()); // length
        raw.extend_from_slice(&0u32.to_be_bytes()); // language
        raw.extend_from_slice(&1u32.to_be_bytes()); // one group
        for v in [0u32, u32::MAX, 1] {
            raw.extend_from_slice(&v.to_be_bytes());
        }
        let cps = Cmap::parse(&raw).unwrap().codepoints();
        assert_eq!(cps.len(), 0x11_0000);
        assert_eq!(cps.last(), Some(&0x10_FFFF));
    }

    /// Format 4 subtable for the segments `(start, end, delta)` plus the 0xFFFF
    /// terminator, in a cmap with one (3,1) record.
    fn format4_cmap(segs: &[(u16, u16, u16)], declared_len: Option<u16>) -> Vec<u8> {
        let n = segs.len() + 1;
        let mut sub = vec![0, 4, 0, 0, 0, 0];
        sub.extend_from_slice(&((n * 2) as u16).to_be_bytes());
        sub.extend_from_slice(&[0; 6]);
        for s in segs {
            sub.extend_from_slice(&s.1.to_be_bytes());
        }
        sub.extend_from_slice(&[0xFF, 0xFF, 0, 0]);
        for s in segs {
            sub.extend_from_slice(&s.0.to_be_bytes());
        }
        sub.extend_from_slice(&[0xFF, 0xFF]);
        for s in segs {
            sub.extend_from_slice(&s.2.to_be_bytes());
        }
        sub.extend_from_slice(&[0, 1]);
        sub.extend_from_slice(&vec![0; n * 2]);
        let len = declared_len.unwrap_or(sub.len() as u16);
        sub[2..4].copy_from_slice(&len.to_be_bytes());
        let mut raw = vec![0, 0, 0, 1, 0, 3, 0, 1, 0, 0, 0, 12];
        raw.extend_from_slice(&sub);
        raw
    }

    #[test]
    fn format_4_maps_through_the_delta() {
        let raw = format4_cmap(&[(0x41, 0x43, 1)], None);
        assert_eq!(
            Cmap::parse(&raw).unwrap().mappings(),
            vec![(0x41, 0x42), (0x42, 0x43), (0x43, 0x44)]
        );
    }

    /// flip_cmap: a subtable declaring more bytes than the cmap holds is dropped, as
    /// Ruby's `parse_format_4` does (read-fonts decoded it anyway: 5 codepoints vs 0).
    #[test]
    fn format_4_declaring_more_than_the_table_holds_maps_nothing() {
        let raw = format4_cmap(&[(0x41, 0x43, 1)], Some(0xFF40));
        assert!(Cmap::parse(&raw).unwrap().mappings().is_empty());
    }

    /// flip_cmap: a segCountX2 larger than the arrays inside the declared length
    /// leaves segments without an idRangeOffset; Ruby raised NoMethodError, both
    /// tiers now skip those segments and keep the complete ones.
    #[test]
    fn format_4_with_a_segment_count_past_its_arrays_skips_the_cut_segments() {
        let mut raw = format4_cmap(&[(0x41, 0x41, 1)], None);
        let x2 = 12 + 6;
        raw[x2..x2 + 2].copy_from_slice(&200u16.to_be_bytes());
        assert!(Cmap::parse(&raw).unwrap().mappings().is_empty());
    }

    #[test]
    fn format_12_declaring_more_than_the_table_holds_maps_nothing() {
        let mut raw = vec![0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12];
        raw.extend_from_slice(&[0, 12, 0, 0]);
        raw.extend_from_slice(&0xFFFFu32.to_be_bytes());
        raw.extend_from_slice(&0u32.to_be_bytes());
        raw.extend_from_slice(&1u32.to_be_bytes());
        for v in [0x41u32, 0x41, 1] {
            raw.extend_from_slice(&v.to_be_bytes());
        }
        assert!(Cmap::parse(&raw).unwrap().mappings().is_empty());
    }
}
