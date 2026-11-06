//! The facade's parsed-table digests, written out on the raw table bytes exactly
//! as `Fontisan::Reference::TableDigests` does (lib/fontisan/reference/table_digests.rb).
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// A structure ran past the end of its table
/// (`Fontisan::Reference::TableDigests::Truncated`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DigestFailure;

impl DigestFailure {
    /// The class name the facade records as `{"error": ...}`.
    pub fn ruby_name(self) -> &'static str {
        "Fontisan::Reference::TableDigests::Truncated"
    }
}

type R<T> = Result<T, DigestFailure>;

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn parts_digest(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(Sha256::digest(p));
    }
    hex(&h.finalize())
}

/// Byte reader over a table; every access is bounds-checked.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn slice(&self, off: u64, len: u64) -> R<&'a [u8]> {
        let end = off.checked_add(len).ok_or(DigestFailure)?;
        if end > self.0.len() as u64 {
            return Err(DigestFailure);
        }
        Ok(&self.0[off as usize..end as usize])
    }
    fn arr<const N: usize>(&self, off: u64) -> R<[u8; N]> {
        Ok(self.slice(off, N as u64)?.try_into().unwrap())
    }
    fn u8(&self, off: u64) -> R<u8> {
        Ok(self.arr::<1>(off)?[0])
    }
    fn u16(&self, off: u64) -> R<u16> {
        Ok(u16::from_be_bytes(self.arr(off)?))
    }
    fn u32(&self, off: u64) -> R<u32> {
        Ok(u32::from_be_bytes(self.arr(off)?))
    }
    fn i32(&self, off: u64) -> R<i32> {
        Ok(i32::from_be_bytes(self.arr(off)?))
    }
    fn version(&self) -> R<(u16, u16)> {
        Ok((self.u16(0)?, self.u16(2)?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GvarDigest {
    pub version: (u16, u16),
    pub axis_count: u16,
    pub flags: u16,
    pub glyph_count: u16,
    pub glyphs_with_data: usize,
    pub glyphs_digest: String,
    pub shared_tuple_count: u16,
    pub shared_tuples_sha256: String,
}

pub(super) fn gvar(data: &[u8]) -> R<GvarDigest> {
    let d = Bytes(data);
    let axis_count = d.u16(4)?;
    let shared_count = d.u16(6)?;
    let shared_offset = d.u32(8)?;
    let glyph_count = d.u16(12)?;
    let flags = d.u16(14)?;
    let array_offset = u64::from(d.u32(16)?);
    let long = flags & 1 != 0;
    let offsets = (0..=u64::from(glyph_count))
        .map(|i| {
            let v = if long {
                u64::from(d.u32(20 + i * 4)?)
            } else {
                u64::from(d.u16(20 + i * 2)?) * 2
            };
            Ok(array_offset + v)
        })
        .collect::<R<Vec<u64>>>()?;
    let parts = offsets
        .windows(2)
        .map(|w| {
            // a descending pair is a negative length in Ruby: Truncated
            let len = w[1].checked_sub(w[0]).ok_or(DigestFailure)?;
            d.slice(w[0], len)
        })
        .collect::<R<Vec<_>>>()?;
    Ok(GvarDigest {
        version: d.version()?,
        axis_count,
        flags,
        glyph_count,
        glyphs_with_data: parts.iter().filter(|p| !p.is_empty()).count(),
        glyphs_digest: parts_digest(&parts),
        shared_tuple_count: shared_count,
        shared_tuples_sha256: sha(d.slice(
            u64::from(shared_offset),
            u64::from(shared_count) * u64::from(axis_count) * 2,
        )?),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IvsDigest {
    pub region_count: u16,
    pub item_data_count: u16,
    pub regions_sha256: String,
    pub data_digest: String,
    pub data_sha256: Vec<String>,
}

fn item_variation_store(d: &Bytes, base: u64) -> R<IvsDigest> {
    let region_offset = u64::from(d.u32(base + 2)?);
    let data_count = d.u16(base + 6)?;
    let region_axes = u64::from(d.u16(base + region_offset)?);
    let region_count = d.u16(base + region_offset + 2)?;
    let region_len = 4 + u64::from(region_count) * region_axes * 6;
    let subtables = (0..u64::from(data_count))
        .map(|i| {
            let start = base + u64::from(d.u32(base + 8 + i * 4)?);
            let items = u64::from(d.u16(start)?);
            let words = d.u16(start + 2)?;
            let indexes = u64::from(d.u16(start + 4)?);
            let word_count = u64::from(words & 0x7FFF);
            let row = if words & 0x8000 != 0 {
                // Ruby's integer arithmetic may go negative; a negative length is Truncated
                (word_count * 4)
                    .checked_add_signed(((indexes as i64) - (word_count as i64)) * 2)
                    .ok_or(DigestFailure)?
            } else {
                word_count + indexes
            };
            d.slice(start, 6 + indexes * 2 + items * row)
        })
        .collect::<R<Vec<_>>>()?;
    Ok(IvsDigest {
        region_count,
        item_data_count: data_count,
        regions_sha256: sha(d.slice(base + region_offset, region_len)?),
        data_digest: parts_digest(&subtables),
        data_sha256: subtables.iter().map(|s| sha(s)).collect(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapDigest {
    pub format: u8,
    pub entry_format: u8,
    pub map_count: u32,
    pub sha256: String,
}

fn delta_set_index_map(d: &Bytes, off: u64) -> R<MapDigest> {
    let format = d.u8(off)?;
    let entry_format = d.u8(off + 1)?;
    let count = if format == 0 {
        u32::from(d.u16(off + 2)?)
    } else {
        d.u32(off + 2)?
    };
    let header = if format == 0 { 4 } else { 6 };
    let size = u64::from((entry_format >> 4) & 3) + 1;
    Ok(MapDigest {
        format,
        entry_format,
        map_count: count,
        sha256: sha(d.slice(off, header + u64::from(count) * size)?),
    })
}

/// HVAR and VVAR share one layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HvarDigest {
    pub version: (u16, u16),
    pub item_variation_store: IvsDigest,
    pub advance_mapping: Option<MapDigest>,
    pub lsb_mapping: Option<MapDigest>,
    pub rsb_mapping: Option<MapDigest>,
}

pub(super) fn hvar(data: &[u8]) -> R<HvarDigest> {
    let d = Bytes(data);
    let version = d.version()?;
    let item_variation_store = item_variation_store(&d, u64::from(d.u32(4)?))?;
    let map = |at: u64| -> R<Option<MapDigest>> {
        match d.u32(at)? {
            0 => Ok(None),
            off => delta_set_index_map(&d, u64::from(off)).map(Some),
        }
    };
    Ok(HvarDigest {
        version,
        item_variation_store,
        advance_mapping: map(8)?,
        lsb_mapping: map(12)?,
        rsb_mapping: map(16)?,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MvarRecord {
    pub tag: String,
    pub outer: u16,
    pub inner: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MvarDigest {
    pub version: (u16, u16),
    pub value_records: Vec<MvarRecord>,
    pub item_variation_store: Option<IvsDigest>,
}

/// The store offset is the spec's Offset16 from the table start (the golden's
/// layout; Ruby's `Tables::Mvar` reads it as uint32 and fails: known_reference_bug).
pub(super) fn mvar(data: &[u8]) -> R<MvarDigest> {
    let d = Bytes(data);
    let record_size = u64::from(d.u16(6)?);
    let count = u64::from(d.u16(8)?);
    let store_offset = d.u16(10)?;
    let value_records = (0..count)
        .map(|i| {
            let at = 12 + i * record_size;
            Ok(MvarRecord {
                tag: d.slice(at, 4)?.iter().map(|&b| char::from(b)).collect(),
                outer: d.u16(at + 4)?,
                inner: d.u16(at + 6)?,
            })
        })
        .collect::<R<Vec<_>>>()?;
    Ok(MvarDigest {
        version: d.version()?,
        value_records,
        item_variation_store: match store_offset {
            0 => None,
            off => Some(item_variation_store(&d, u64::from(off))?),
        },
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatAxis {
    pub tag: String,
    pub name_id: u16,
    pub ordering: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AxisValue {
    /// Formats 1-3: `values` are `(name, raw Fixed)` in table order
    /// (`value`; `nominal`, `range_min`, `range_max`; `value`, `linked_value`).
    Fixed {
        format: u16,
        axis_index: u16,
        flags: u16,
        value_name_id: u16,
        values: Vec<(&'static str, i32)>,
    },
    /// Format 4: `(axis_index, raw Fixed)` pairs.
    Multi {
        flags: u16,
        value_name_id: u16,
        axis_values: Vec<(u16, i32)>,
    },
    Unsupported {
        format: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatDigest {
    pub version: (u16, u16),
    pub elided_fallback_name_id: Option<u16>,
    pub axes: Vec<StatAxis>,
    pub axis_values: Vec<AxisValue>,
}

pub(super) fn stat(data: &[u8]) -> R<StatDigest> {
    let d = Bytes(data);
    let axis_size = u64::from(d.u16(4)?);
    let axis_count = u64::from(d.u16(6)?);
    let axes_offset = u64::from(d.u32(8)?);
    let value_count = u64::from(d.u16(12)?);
    let values_offset = u64::from(d.u32(14)?);
    let minor = d.u16(2)?;
    let elided = if minor >= 1 { Some(d.u16(18)?) } else { None };
    let axes = (0..axis_count)
        .map(|i| {
            let at = axes_offset + i * axis_size;
            Ok(StatAxis {
                tag: d.slice(at, 4)?.iter().map(|&b| char::from(b)).collect(),
                name_id: d.u16(at + 4)?,
                ordering: d.u16(at + 6)?,
            })
        })
        .collect::<R<Vec<_>>>()?;
    let axis_values = (0..value_count)
        .map(|i| {
            let rel = u64::from(d.u16(values_offset + i * 2)?);
            axis_value(&d, values_offset + rel)
        })
        .collect::<R<Vec<_>>>()?;
    Ok(StatDigest {
        version: d.version()?,
        elided_fallback_name_id: elided,
        axes,
        axis_values,
    })
}

fn axis_value(d: &Bytes, pos: u64) -> R<AxisValue> {
    let format = d.u16(pos)?;
    let fixed = |names: &[&'static str]| -> R<AxisValue> {
        Ok(AxisValue::Fixed {
            format,
            axis_index: d.u16(pos + 2)?,
            flags: d.u16(pos + 4)?,
            value_name_id: d.u16(pos + 6)?,
            values: names
                .iter()
                .enumerate()
                .map(|(i, n)| Ok((*n, d.i32(pos + 8 + i as u64 * 4)?)))
                .collect::<R<Vec<_>>>()?,
        })
    };
    match format {
        1 => fixed(&["value"]),
        2 => fixed(&["nominal", "range_min", "range_max"]),
        3 => fixed(&["value", "linked_value"]),
        4 => {
            let n = u64::from(d.u16(pos + 2)?);
            Ok(AxisValue::Multi {
                flags: d.u16(pos + 4)?,
                value_name_id: d.u16(pos + 6)?,
                axis_values: (0..n)
                    .map(|j| Ok((d.u16(pos + 8 + j * 6)?, d.i32(pos + 10 + j * 6)?)))
                    .collect::<R<Vec<_>>>()?,
            })
        }
        _ => Ok(AxisValue::Unsupported { format }),
    }
}
