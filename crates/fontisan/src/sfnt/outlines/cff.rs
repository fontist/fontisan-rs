//! CFF (version 1) CharStrings, walked the way Ruby's `Tables::Cff` does:
//! header, Name/Top DICT/String/Global Subr INDEXes, then Top DICT operator 17.
use crate::sfnt::error::{Error, ErrorClass};

fn corrupted(msg: impl Into<String>) -> Error {
    Error::new(ErrorClass::CorruptedTable, msg)
}

/// A parsed INDEX: per-object `(start, end)` ranges into the table and the
/// position just past the INDEX.
pub(super) struct Index {
    pub items: Vec<(usize, usize)>,
    pub end: usize,
}

const END: &str = "Unexpected end of INDEX data";

fn take(data: &[u8], pos: usize, n: usize) -> Result<&[u8], Error> {
    if n == 0 {
        return Ok(&[]);
    }
    data.get(pos..)
        .and_then(|r| r.get(..n))
        .ok_or_else(|| corrupted(END))
}

pub(super) fn index(data: &[u8], start: usize) -> Result<Index, Error> {
    let count = usize::from(u16::from_be_bytes(
        take(data, start, 2)?.try_into().unwrap(),
    ));
    if count == 0 {
        return Ok(Index {
            items: vec![],
            end: start + 2,
        });
    }
    let off_size = usize::from(take(data, start + 2, 1)?[0]);
    if !(1..=4).contains(&off_size) {
        return Err(corrupted(format!(
            "Invalid INDEX offSize: {off_size} (must be 1-4)"
        )));
    }
    let mut offsets = Vec::with_capacity(count + 1);
    for i in 0..=count {
        let b = take(data, start + 3 + i * off_size, off_size)?;
        offsets.push(b.iter().fold(0usize, |a, &x| (a << 8) | usize::from(x)));
    }
    if offsets[0] != 1 {
        return Err(corrupted(format!(
            "Invalid INDEX: first offset must be 1, got {}",
            offsets[0]
        )));
    }
    if offsets.windows(2).any(|w| w[1] < w[0]) {
        return Err(corrupted(
            "Invalid INDEX: offsets are not in ascending order",
        ));
    }
    let data_start = start + 3 + (count + 1) * off_size;
    let data_size = offsets[count] - 1;
    take(data, data_start, data_size)?;
    Ok(Index {
        items: offsets
            .windows(2)
            .map(|w| (data_start + w[0] - 1, data_start + w[1] - 1))
            .collect(),
        end: data_start + data_size,
    })
}

/// Operator 17 (CharStrings offset) in a CFF1 DICT, the operand Ruby stores.
fn charstrings_offset(dict: &[u8]) -> Result<Option<i64>, Error> {
    let mut stack: Vec<i64> = vec![];
    let mut i = 0;
    let mut found = None;
    while i < dict.len() {
        let b = dict[i];
        i += 1;
        if b <= 21 {
            let op = if b == 12 {
                // Ruby reads the escape's second byte with read_byte (EOF raises).
                let b2 = *dict
                    .get(i)
                    .ok_or_else(|| corrupted("Unexpected end of DICT"))?;
                i += 1;
                1200 + u32::from(b2)
            } else {
                u32::from(b)
            };
            if op == 17 {
                found = stack.first().copied();
            }
            stack.clear();
            continue;
        }
        let need = |n: usize| {
            dict.get(i..i + n)
                .ok_or_else(|| corrupted("Unexpected end of DICT"))
        };
        let v = match b {
            28 => {
                let s = need(2)?;
                i += 2;
                i64::from(i16::from_be_bytes([s[0], s[1]]))
            }
            29 => {
                let s = need(4)?;
                i += 4;
                i64::from(i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
            }
            30 => {
                // Real: nibbles until an 0xF nibble; the value is irrelevant here.
                loop {
                    let n = *dict
                        .get(i)
                        .ok_or_else(|| corrupted("Unexpected end of DICT"))?;
                    i += 1;
                    if n >> 4 == 0xF || n & 0xF == 0xF {
                        break;
                    }
                }
                0
            }
            32..=246 => i64::from(b) - 139,
            247..=250 => {
                let s = need(1)?;
                i += 1;
                (i64::from(b) - 247) * 256 + i64::from(s[0]) + 108
            }
            251..=254 => {
                let s = need(1)?;
                i += 1;
                -(i64::from(b) - 251) * 256 - i64::from(s[0]) - 108
            }
            _ => return Err(corrupted(format!("Invalid DICT operand byte: {b}"))),
        };
        stack.push(v);
    }
    Ok(found)
}

pub(super) fn glyphs(data: &[u8]) -> Result<Vec<&[u8]>, Error> {
    parse(data).map_err(|e| {
        if e.class == ErrorClass::CorruptedTable && !e.message.starts_with("Failed to parse") {
            corrupted(format!("Failed to parse CFF table: {}", e.message))
        } else {
            e
        }
    })
}

fn parse(data: &[u8]) -> Result<Vec<&[u8]>, Error> {
    let h = take(data, 0, 4)
        .map_err(|_| corrupted("Failed to parse CFF table: Unexpected end of data"))?;
    let (major, minor, hdr_size, off_size) = (h[0], h[1], h[2], h[3]);
    if !(major == 1 || major == 2) || minor != 0 || hdr_size < 4 || !(1..=4).contains(&off_size) {
        return Err(corrupted(format!(
            "Invalid CFF header: version={major}.{minor}, hdr_size={hdr_size}, off_size={off_size}"
        )));
    }
    let names = index(data, usize::from(hdr_size))?;
    if names.items.is_empty() {
        return Err(corrupted("CFF table must contain at least one font"));
    }
    let tops = index(data, names.end)?;
    if tops.items.len() != names.items.len() {
        return Err(corrupted(format!(
            "Top DICT count ({}) must match Name count ({})",
            tops.items.len(),
            names.items.len()
        )));
    }
    let strings = index(data, tops.end)?;
    index(data, strings.end)?;
    // Ruby parses every Top DICT eagerly; any bad operand fails the whole table.
    let mut cs = None;
    for (n, &(s, e)) in tops.items.iter().enumerate() {
        let off = charstrings_offset(&data[s..e])?;
        if n == 0 {
            cs = off;
        }
    }
    let Some(off) = cs else { return Ok(vec![]) };
    let wrap = |e: Error| corrupted(format!("Failed to parse CharStrings INDEX: {}", e.message));
    let off = usize::try_from(off)
        .map_err(|_| corrupted("Failed to parse CharStrings INDEX: negative seek"))?;
    let ix = index(data, off).map_err(wrap)?;
    Ok(ix.items.iter().map(|&(s, e)| &data[s..e]).collect())
}
