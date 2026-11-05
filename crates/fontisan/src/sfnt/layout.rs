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

/// `LangSys`: reserved u16, required index u16, count u16, `count` x u16.
fn lang_sys(data: &[u8]) -> Result<Vec<u16>, Error> {
    let mut c = Cursor::new(data, 0);
    c.u16()?;
    c.u16()?;
    let count = c.u16()?;
    let mut v = Vec::with_capacity(usize::from(count).min(c.rest().len() / 2));
    for _ in 0..count {
        v.push(c.u16()?);
    }
    Ok(v)
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
        let mut indices = Vec::new();
        if default_offset != 0 {
            if let Some(d) = non_empty(data, i64::from(default_offset)) {
                indices.extend(lang_sys(d)?);
            }
        }
        for (_, off) in langs {
            if let Some(d) = non_empty(data, i64::from(off)) {
                indices.extend(lang_sys(d)?);
            }
        }
        let mut seen = std::collections::HashSet::new();
        indices.retain(|i| seen.insert(*i));
        Ok(indices)
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
        for idx in indices {
            if let Some((tag, _)) = features.get(usize::from(idx)) {
                if !out.contains(tag) {
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
