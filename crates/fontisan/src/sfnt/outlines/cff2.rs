//! CFF2 CharStrings, located through Top DICT operator 17 with the spec layout
//! (uint32 INDEX count). Mirrors the facade's `TableDigests.cff2_charstrings`,
//! not Ruby's `Tables::Cff2` (TRIAGE: CFF2-PARSER).
use crate::sfnt::error::{Error, ErrorClass};

fn slice(data: &[u8], offset: i64, length: i64) -> Result<&[u8], Error> {
    let bad = || {
        Error::new(
            ErrorClass::Truncated,
            format!("{length} bytes at {offset} past {}", data.len()),
        )
    };
    if offset < 0 || length < 0 {
        return Err(bad());
    }
    let (o, l) = (offset as usize, length as usize);
    o.checked_add(l)
        .filter(|&end| end <= data.len())
        .map(|end| &data[o..end])
        .ok_or_else(bad)
}

fn be(b: &[u8]) -> i64 {
    b.iter().fold(0i64, |a, &x| (a << 8) | i64::from(x))
}

/// The last operand before `operator`, or None. Real numbers are skipped.
fn dict_operand(dict: &[u8], operator: u32) -> Result<Option<i64>, Error> {
    let nomethod = || Error::new(ErrorClass::NoMethod, "undefined method for nil");
    let mut stack: Vec<i64> = vec![];
    let mut i = 0usize;
    while i < dict.len() {
        let lead = dict[i];
        if lead <= 21 {
            let op = if lead == 12 {
                i += 1;
                1200 + u32::from(*dict.get(i).ok_or_else(nomethod)?)
            } else {
                u32::from(lead)
            };
            if op == operator {
                return Ok(stack.last().copied());
            }
            stack.clear();
        } else {
            let arg = |n: usize| dict.get(i + 1..i + 1 + n).ok_or_else(nomethod);
            match lead {
                28 => {
                    let s = arg(2)?;
                    stack.push(i64::from(i16::from_be_bytes([s[0], s[1]])));
                    i += 2;
                }
                29 => {
                    let s = arg(4)?;
                    stack.push(i64::from(i32::from_be_bytes([s[0], s[1], s[2], s[3]])));
                    i += 4;
                }
                30 => {
                    i += 1;
                    while i < dict.len() && dict[i] >> 4 != 0xF && dict[i] & 0xF != 0xF {
                        i += 1;
                    }
                    stack.push(0);
                }
                32..=246 => stack.push(i64::from(lead) - 139),
                247..=250 => {
                    stack.push((i64::from(lead) - 247) * 256 + i64::from(arg(1)?[0]) + 108);
                    i += 1;
                }
                251..=254 => {
                    stack.push(-(i64::from(lead) - 251) * 256 - i64::from(arg(1)?[0]) - 108);
                    i += 1;
                }
                _ => stack.push(0),
            }
        }
        i += 1;
    }
    Ok(None)
}

pub(super) fn glyphs(data: &[u8]) -> Result<Vec<&[u8]>, Error> {
    let header_size = be(slice(data, 2, 1)?);
    let top_length = be(slice(data, 3, 2)?);
    let top = slice(data, header_size, top_length)?;
    let Some(offset) = dict_operand(top, 17)? else {
        return Ok(vec![]);
    };
    let count = be(slice(data, offset, 4)?);
    if count == 0 {
        return Ok(vec![]);
    }
    // Every charstring needs at least one offset byte, so more entries than bytes
    // cannot be real. Without this, offSize 0 (zero-width reads always succeed)
    // plus a huge count loops ~4 billion times (fuzz finding, 1j-parsers).
    if count > data.len() as i64 {
        return Err(Error::new(
            ErrorClass::Truncated,
            format!("{count} CharStrings in a {} byte table", data.len()),
        ));
    }
    let off_size = be(slice(data, offset + 4, 1)?);
    let read = |i: i64| -> Result<i64, Error> {
        Ok(be(slice(data, offset + 5 + i * off_size, off_size)?))
    };
    let base = offset + 5 + (count + 1) * off_size - 1;
    (0..count)
        .map(|i| {
            let (a, b) = (read(i)?, read(i + 1)?);
            // Offsets are up to 255 bytes wide: the sums overflow i64 without checks
            // (a panic in a debug build, a wrapped slice in release).
            let overflow = || {
                Error::new(
                    ErrorClass::Truncated,
                    format!("CharString offsets {a}..{b} overflow"),
                )
            };
            slice(
                data,
                base.checked_add(a).ok_or_else(overflow)?,
                b.checked_sub(a).ok_or_else(overflow)?,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CharStrings INDEX with 8-byte offsets near i64::MAX must fail, not overflow.
    #[test]
    fn huge_offsets_fail_without_overflow() {
        for (first, second) in [(i64::MAX, 0), (0, i64::MIN), (i64::MIN, i64::MAX)] {
            let mut d = vec![2, 0, 5, 0, 3, 139 + 8, 139 + 8, 17];
            d.extend_from_slice(&1u32.to_be_bytes()); // count
            d.push(8); // offSize
            d.extend_from_slice(&first.to_be_bytes());
            d.extend_from_slice(&second.to_be_bytes());
            assert!(glyphs(&d).is_err(), "{first} {second}");
        }
    }

    /// fuzz finding: CharStrings INDEX with count 0xFFFFFFFF and offSize 0 never ended.
    #[test]
    fn huge_count_with_zero_off_size_fails_fast() {
        // Header: major 2, minor 0, headerSize 5, topDictLength 3. Top DICT at 5:
        // operand 8, operand 8, operator 17 (CharStrings offset 8, where the INDEX starts).
        let mut d = vec![2, 0, 5, 0, 3, 139 + 8, 139 + 8, 17];
        d.extend_from_slice(&u32::MAX.to_be_bytes()); // count
        d.push(0); // offSize 0
        d.extend_from_slice(&[0; 8]);
        assert_eq!(glyphs(&d).unwrap_err().class, ErrorClass::Truncated);
    }
}
