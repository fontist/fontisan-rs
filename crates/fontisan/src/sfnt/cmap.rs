//! `cmap` as Ruby reads it (`Fontisan::Tables::Cmap`): ONE subtable chosen by
//! (3,10), then (3,1), then the first platform-0 record, only formats 4 and 12
//! are decoded, and gid 0 is dropped. read-fonts decodes the subtables; the
//! selection is Ruby's own, not skrifa's `Charmap` (docs/substrate.md, 1b).
use super::cursor::Cursor;
use super::error::Error;
use read_fonts::tables::cmap::CmapSubtable;
use read_fonts::{FontData, FontRead};

const MAX_CODEPOINT: u32 = 0x10_FFFF;
const MAX_ENTRIES: usize = MAX_CODEPOINT as usize + 1;

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
        if sub.len() < 2 || !matches!(u16::from_be_bytes([sub[0], sub[1]]), 4 | 12) {
            return Vec::new();
        }
        let Ok(table) = CmapSubtable::read(FontData::new(sub)) else {
            return Vec::new();
        };
        // A format 12 group may claim 0..=0xFFFF_FFFF; walking it unbounded hangs and
        // exhausts memory. No valid cmap maps more than the 0x110000 Unicode scalars,
        // so stop after that many entries and past U+10FFFF (valid fonts: unchanged).
        let mut m: Vec<(u32, u32)> = table
            .iter()
            .take(MAX_ENTRIES)
            .take_while(|(c, _)| *c <= MAX_CODEPOINT)
            .filter(|(_, g)| g.to_u32() != 0)
            .map(|(c, g)| (c, g.to_u32()))
            .collect();
        m.sort_unstable();
        m.dedup_by_key(|(c, _)| *c);
        m
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
}
