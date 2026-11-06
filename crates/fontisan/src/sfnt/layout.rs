//! GSUB / GPOS script and feature enumeration (card 1f-core), reproducing the
//! Ruby reference's `Tables::Gsub` / `Tables::Gpos` quirks:
//!
//! * the header is five u16, then `table_data` is everything after it, and every
//!   offset is table-relative, so a slice is `table_data[(off - 10)..]` with Ruby
//!   `String#[]` semantics (a negative start counts from the END, a start past
//!   the end is `nil`, a start at the end is `""`);
//! * every failure inside `scripts` / `features` (a short read) yields `[]`, and
//!   partial results are lost;
//! * building the table itself fails on fewer than 10 bytes (as the other 1c
//!   records do) and an empty table is the all-zero record.
use super::cursor::Cursor;
use super::error::Error;
use super::font::Font;

/// A four-byte OpenType tag, compared and sorted as raw bytes (Ruby binary strings).
pub type Tag = [u8; 4];

const HEADER: usize = 10;

/// `GSUB` or `GPOS` (identical layouts for what the audit reads).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayoutTable {
    pub major_version: u16,
    pub minor_version: u16,
    pub script_list_offset: u16,
    pub feature_list_offset: u16,
    pub lookup_list_offset: u16,
    table_data: Vec<u8>,
}

/// Ruby `data[start..]`: `None` when the start is out of range.
fn rb_from(data: &[u8], start: i64) -> Option<&[u8]> {
    let len = data.len() as i64;
    let at = if start < 0 { len + start } else { start };
    if at < 0 || at > len {
        None
    } else {
        Some(&data[at as usize..])
    }
}

/// Like `rb_from`, but also `None` for an empty slice (the Ruby `nil? || empty?` guard).
fn non_empty(data: &[u8], start: i64) -> Option<&[u8]> {
    rb_from(data, start).filter(|s| !s.is_empty())
}

fn read_records(c: &mut Cursor, count: u16) -> Result<Vec<(Tag, u16)>, Error> {
    let mut v = Vec::with_capacity(usize::from(count).min(c.rest().len() / 6));
    for _ in 0..count {
        let tag: Tag = c.bytes(4)?.try_into().unwrap();
        v.push((tag, c.u16()?));
    }
    Ok(v)
}

/// `ScriptList` / `FeatureList`: u16 count, then `count` x (tag, offset).
fn tag_list(data: &[u8]) -> Result<Vec<(Tag, u16)>, Error> {
    let mut c = Cursor::new(data, 0);
    let count = c.u16()?;
    read_records(&mut c, count)
}

/// Collects the distinct feature indices of many `LangSys` tables in first-seen
/// order without re-reading shared bytes.
///
/// A `LangSys` index array is a run of u16 words at a fixed byte parity inside
/// the script table, so overlapping arrays (a hostile font can point tens of
/// thousands of records at overlapping 64K-entry arrays) read the same words
/// again and again. Each word is read once: `skip` is a "next unread
/// slot" union-find per parity (slot = byte position / 2). A skipped word was
/// already collected earlier, so the result and its first-seen order are
/// exactly those of reading every array in full.
struct IndexCollector {
    /// `skip[parity][slot]`: 0 while the slot is unread, else the next slot to try.
    skip: [Vec<u32>; 2],
    seen: Vec<bool>,
    out: Vec<u16>,
}

impl IndexCollector {
    fn new(len: usize) -> Self {
        let slots = len / 2 + 2;
        Self {
            skip: [vec![0; slots], vec![0; slots]],
            seen: vec![false; 1 << 16],
            out: Vec::new(),
        }
    }

    fn find(&mut self, parity: usize, slot: usize) -> usize {
        let skip = &mut self.skip[parity];
        let mut root = slot;
        while skip[root] != 0 {
            root = skip[root] as usize;
        }
        let mut cur = slot;
        while cur != root {
            let next = skip[cur] as usize;
            skip[cur] = root as u32;
            cur = next;
        }
        root
    }

    /// `LangSys`: reserved u16, required index u16, count u16, `count` x u16,
    /// read at `data[offset..]`; `Err` on a short read, as `Cursor` would.
    fn add(&mut self, data: &[u8], offset: usize) -> Result<(), Error> {
        let mut c = Cursor::new(&data[offset..], 0);
        c.u16()?;
        c.u16()?;
        let count = usize::from(c.u16()?);
        c.bytes(count * 2)?;
        let first = offset + 6;
        let parity = first & 1;
        let (lo, hi) = (first >> 1, (first + 2 * count) >> 1);
        let mut slot = self.find(parity, lo);
        while slot < hi {
            let pos = slot * 2 + parity;
            let index = u16::from_be_bytes([data[pos], data[pos + 1]]);
            if !std::mem::replace(&mut self.seen[usize::from(index)], true) {
                self.out.push(index);
            }
            self.skip[parity][slot] = (slot + 1) as u32;
            slot = self.find(parity, slot + 1);
        }
        Ok(())
    }
}

impl LayoutTable {
    /// Parse a table body (the bytes `font.table("GSUB")` holds).
    pub fn from_bytes(data: &[u8]) -> Result<Self, Error> {
        if data.is_empty() {
            return Ok(Self::default());
        }
        let mut c = Cursor::new(data, 0);
        Ok(Self {
            major_version: c.u16()?,
            minor_version: c.u16()?,
            script_list_offset: c.u16()?,
            feature_list_offset: c.u16()?,
            lookup_list_offset: c.u16()?,
            table_data: c.rest().to_vec(),
        })
    }

    fn script_list(&self) -> Option<&[u8]> {
        non_empty(
            &self.table_data,
            i64::from(self.script_list_offset) - HEADER as i64,
        )
    }

    /// Script tags in record order (duplicates kept); `[]` on any failure.
    pub fn scripts(&self) -> Vec<Tag> {
        if self.script_list_offset == 0 {
            return vec![];
        }
        self.script_list()
            .and_then(|d| tag_list(d).ok())
            .map(|r| r.into_iter().map(|(t, _)| t).collect())
            .unwrap_or_default()
    }

    /// Feature indices of the first record for `script`, over its default
    /// LangSys and every LangSys record; `[]` on any failure.
    fn feature_indices(&self, script: &Tag) -> Vec<u16> {
        self.try_feature_indices(script).unwrap_or_default()
    }

    fn try_feature_indices(&self, script: &Tag) -> Result<Vec<u16>, Error> {
        let Some(list) = self.script_list() else {
            return Ok(vec![]);
        };
        let scripts = tag_list(list)?;
        let Some(&(_, offset)) = scripts.iter().find(|(t, _)| t == script) else {
            return Ok(vec![]);
        };
        let Some(data) = non_empty(list, i64::from(offset)) else {
            return Ok(vec![]);
        };
        let mut c = Cursor::new(data, 0);
        let default_offset = c.u16()?;
        let count = c.u16()?;
        let langs = read_records(&mut c, count)?;
        let mut indices = IndexCollector::new(data.len());
        if default_offset != 0 {
            if let Some(d) = non_empty(data, i64::from(default_offset)) {
                indices.add(data, data.len() - d.len())?;
            }
        }
        for (_, off) in langs {
            if let Some(d) = non_empty(data, i64::from(off)) {
                indices.add(data, data.len() - d.len())?;
            }
        }
        Ok(indices.out)
    }

    /// Feature tags enabled for `script` (unique, in index order); `[]` on any failure.
    pub fn features(&self, script: &Tag) -> Vec<Tag> {
        if self.script_list_offset == 0 || self.feature_list_offset == 0 {
            return vec![];
        }
        let indices = self.feature_indices(script);
        if indices.is_empty() {
            return vec![];
        }
        let Some(list) = non_empty(
            &self.table_data,
            i64::from(self.feature_list_offset) - HEADER as i64,
        ) else {
            return vec![];
        };
        let Ok(features) = tag_list(list) else {
            return vec![];
        };
        let mut out: Vec<Tag> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for idx in indices {
            if let Some((tag, _)) = features.get(usize::from(idx)) {
                if seen.insert(*tag) {
                    out.push(*tag);
                }
            }
        }
        out
    }
}

impl Font<'_> {
    /// `font.table("GSUB")`: `Err` when the mode gates it out or it is too short.
    pub fn gsub(&self) -> Result<Option<LayoutTable>, Error> {
        self.parsed("GSUB", LayoutTable::from_bytes)
    }

    /// `font.table("GPOS")`.
    pub fn gpos(&self) -> Result<Option<LayoutTable>, Error> {
        self.parsed("GPOS", LayoutTable::from_bytes)
    }
}
