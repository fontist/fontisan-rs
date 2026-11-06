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
use std::collections::{HashMap, HashSet};

use super::cursor::Cursor;
use super::error::Error;
use super::font::Font;

/// A four-byte OpenType tag, compared and sorted as raw bytes (Ruby binary strings).
pub type Tag = [u8; 4];

/// One script tag with its feature tags.
pub type ScriptFeatures = Vec<(Tag, Vec<Tag>)>;

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

/// `count` x (tag, offset). The whole run is length-checked up front, so a
/// hostile count fails in O(1) instead of after reading every remaining byte.
fn read_records(c: &mut Cursor, count: u16) -> Result<Vec<(Tag, u16)>, Error> {
    let run = c.bytes(usize::from(count) * 6)?;
    Ok(run
        .chunks_exact(6)
        .map(|r| (r[..4].try_into().unwrap(), u16::from_be_bytes([r[4], r[5]])))
        .collect())
}

/// `ScriptList` / `FeatureList`: u16 count, then `count` x (tag, offset).
fn tag_list(data: &[u8]) -> Result<Vec<(Tag, u16)>, Error> {
    let mut c = Cursor::new(data, 0);
    let count = c.u16()?;
    read_records(&mut c, count)
}

/// Work units one `script_features` call may spend, per byte of table. A
/// well-formed font spends a small fraction of one unit per byte; only a table
/// whose scripts each re-walk thousands of LangSys records (every script is
/// otherwise independent, so the work is scripts x table length) can exhaust
/// it, and then the remaining scripts read as "failed" (`[]`), the same
/// outcome as any other bad read.
const WORK_PER_BYTE: usize = 2;

/// Spends `n` units; `Err` (a failed read) once the budget is gone.
fn spend(budget: &mut usize, n: usize) -> Result<(), Error> {
    match budget.checked_sub(n) {
        Some(left) => {
            *budget = left;
            Ok(())
        }
        None => Cursor::new(&[], 0).u16().map(|_| ()),
    }
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
///
/// All state is sparse (hash maps keyed by the slots actually touched), so
/// building one costs nothing and a collector's memory is proportional to the
/// words it read, never to the table length: a table with tens of thousands of
/// scripts must not pay a table-sized allocation per script.
#[derive(Default)]
struct IndexCollector {
    /// `skip[parity][slot]`: absent while the slot is unread, else the next slot to try.
    skip: [HashMap<usize, usize>; 2],
    seen: HashSet<u16>,
    out: Vec<u16>,
}

impl IndexCollector {
    fn find(&mut self, parity: usize, slot: usize) -> usize {
        let skip = &mut self.skip[parity];
        let mut root = slot;
        while let Some(&next) = skip.get(&root) {
            root = next;
        }
        let mut cur = slot;
        while cur != root {
            let next = skip[&cur];
            skip.insert(cur, root);
            cur = next;
        }
        root
    }

    /// `LangSys`: reserved u16, required index u16, count u16, `count` x u16,
    /// read at `data[offset..]`; `Err` on a short read, as `Cursor` would.
    fn add(&mut self, data: &[u8], offset: usize, budget: &mut usize) -> Result<(), Error> {
        let mut c = Cursor::new(&data[offset..], 0);
        c.u16()?;
        c.u16()?;
        let count = usize::from(c.u16()?);
        c.bytes(count * 2)?;
        spend(budget, 1)?;
        let first = offset + 6;
        let parity = first & 1;
        let (lo, hi) = (first >> 1, (first + 2 * count) >> 1);
        let mut slot = self.find(parity, lo);
        while slot < hi {
            let pos = slot * 2 + parity;
            spend(budget, 1)?;
            let index = u16::from_be_bytes([data[pos], data[pos + 1]]);
            if self.seen.insert(index) {
                self.out.push(index);
            }
            self.skip[parity].insert(slot, slot + 1);
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

    /// Feature indices of one script record: its default LangSys and every
    /// LangSys record, deduplicated in first-seen order. `Err` on a short read.
    fn feature_indices(list: &[u8], offset: u16, budget: &mut usize) -> Result<Vec<u16>, Error> {
        let Some(data) = non_empty(list, i64::from(offset)) else {
            return Ok(vec![]);
        };
        let mut c = Cursor::new(data, 0);
        let default_offset = c.u16()?;
        let count = c.u16()?;
        spend(budget, usize::from(count))?;
        let langs = read_records(&mut c, count)?;
        let mut indices = IndexCollector::default();
        if default_offset != 0 {
            if let Some(d) = non_empty(data, i64::from(default_offset)) {
                indices.add(data, data.len() - d.len(), budget)?;
            }
        }
        for (_, off) in langs {
            if let Some(d) = non_empty(data, i64::from(off)) {
                indices.add(data, data.len() - d.len(), budget)?;
            }
        }
        Ok(indices.out)
    }

    /// Feature tags of one script record (unique, in index order); `[]` on any failure.
    fn record_features(
        list: &[u8],
        offset: u16,
        features: &[(Tag, u16)],
        budget: &mut usize,
    ) -> Vec<Tag> {
        let indices = Self::feature_indices(list, offset, budget).unwrap_or_default();
        let mut seen = HashSet::new();
        indices
            .into_iter()
            .filter_map(|i| features.get(usize::from(i)))
            .filter(|(tag, _)| seen.insert(*tag))
            .map(|&(tag, _)| tag)
            .collect()
    }

    /// The parsed FeatureList records, `None` when `features()` must yield `[]`.
    fn feature_records(&self) -> Option<Vec<(Tag, u16)>> {
        if self.script_list_offset == 0 || self.feature_list_offset == 0 {
            return None;
        }
        let list = non_empty(
            &self.table_data,
            i64::from(self.feature_list_offset) - HEADER as i64,
        )?;
        tag_list(list).ok()
    }

    /// Feature tags enabled for `script` (unique, in index order); `[]` on any failure.
    pub fn features(&self, script: &Tag) -> Vec<Tag> {
        let (Some(features), Some(list)) = (self.feature_records(), self.script_list()) else {
            return vec![];
        };
        let Ok(scripts) = tag_list(list) else {
            return vec![];
        };
        // One script is bounded by the table itself (<= length/6 records and
        // <= length/2 distinct words), so a lone lookup needs no budget.
        let mut unlimited = usize::MAX;
        match scripts.iter().find(|(t, _)| t == script) {
            Some(&(_, offset)) => Self::record_features(list, offset, &features, &mut unlimited),
            None => vec![],
        }
    }

    /// `scripts()` paired with `features(script)` for each, in record order
    /// (duplicates kept). The ScriptList and FeatureList are parsed once and
    /// each distinct tag is resolved once (the first record of a tag decides),
    /// and the total LangSys work is capped at `WORK_PER_BYTE` x the table
    /// length (see there), so the cost is linear in the table, not
    /// scripts x table length as calling `features` per script would be.
    pub fn script_features(&self) -> ScriptFeatures {
        self.script_features_metered().0
    }

    /// `script_features()` plus `(units used, budget)`; lets a test assert how
    /// far real fonts sit from the cutoff.
    #[doc(hidden)]
    pub fn script_features_metered(&self) -> (ScriptFeatures, (usize, usize)) {
        let total = self.table_data.len().saturating_mul(WORK_PER_BYTE);
        if self.script_list_offset == 0 {
            return (vec![], (0, total));
        }
        let Some(list) = self.script_list() else {
            return (vec![], (0, total));
        };
        let Ok(scripts) = tag_list(list) else {
            return (vec![], (0, total));
        };
        let features = self.feature_records();
        let mut memo: HashMap<Tag, Vec<Tag>> = HashMap::new();
        let mut budget = total;
        let out = scripts
            .iter()
            .map(|&(tag, offset)| {
                let f = memo.entry(tag).or_insert_with(|| match &features {
                    Some(fs) => Self::record_features(list, offset, fs, &mut budget),
                    None => vec![],
                });
                (tag, f.clone())
            })
            .collect();
        (out, (total - budget, total))
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
